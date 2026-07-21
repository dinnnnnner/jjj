use crate::*;
use egui_plot::{Line, Plot, PlotPoint, PlotPoints, Points};
use futures_util::{TryStreamExt, pin_mut};
use std::fs;
use std::io::Write;
use std::thread;
use tokio_postgres::NoTls;

#[derive(Clone, Copy)]
struct ReplayBucket {
    min: PlotPoint,
    max: PlotPoint,
}

/// One-pass time-bucket sampler. When downsampling is enabled, memory stays
/// bounded even if PostgreSQL returns millions of rows, while the min/max
/// envelope retains short spikes. When disabled, every point is retained.
struct ReplaySeriesSampler {
    start_ts_ms: i64,
    span_ms: i64,
    max_points: usize,
    downsampling_enabled: bool,
    exact_points: Option<Vec<PlotPoint>>,
    buckets: Vec<Option<ReplayBucket>>,
    first: Option<PlotPoint>,
    last: Option<PlotPoint>,
}

impl ReplaySeriesSampler {
    fn new(
        start_ts_ms: i64,
        end_ts_ms: i64,
        max_points: usize,
        downsampling_enabled: bool,
    ) -> Self {
        let bucket_count = max_points.saturating_sub(2).div_euclid(2).max(1);
        Self {
            start_ts_ms,
            span_ms: end_ts_ms.saturating_sub(start_ts_ms).max(1),
            max_points,
            downsampling_enabled,
            exact_points: Some(Vec::with_capacity(if downsampling_enabled {
                max_points.saturating_add(1)
            } else {
                4096
            })),
            buckets: vec![None; bucket_count],
            first: None,
            last: None,
        }
    }

    fn push(&mut self, ts_ms: i64, value: f64) {
        let point = PlotPoint::new(
            ts_ms.saturating_sub(self.start_ts_ms) as f64 / 1000.0,
            value,
        );
        if let Some(exact_points) = &mut self.exact_points {
            exact_points.push(point);
            if !self.downsampling_enabled || exact_points.len() <= self.max_points {
                return;
            }

            let buffered = self.exact_points.take().unwrap_or_default();
            for buffered_point in buffered {
                self.push_sampled(buffered_point);
            }
            return;
        }

        self.push_sampled(point);
    }

    fn push_sampled(&mut self, point: PlotPoint) {
        self.first.get_or_insert(point);
        self.last = Some(point);

        let offset_ms = (point.x * 1000.0).clamp(0.0, self.span_ms as f64);
        let bucket_index = (((offset_ms as f64 / self.span_ms as f64) * self.buckets.len() as f64)
            as usize)
            .min(self.buckets.len() - 1);
        match &mut self.buckets[bucket_index] {
            Some(bucket) => {
                if point.y < bucket.min.y {
                    bucket.min = point;
                }
                if point.y > bucket.max.y {
                    bucket.max = point;
                }
            }
            slot @ None => {
                *slot = Some(ReplayBucket {
                    min: point,
                    max: point,
                });
            }
        }
    }

    fn finish(self) -> Vec<PlotPoint> {
        if let Some(exact_points) = self.exact_points {
            return exact_points;
        }

        let mut points = Vec::with_capacity(self.buckets.len() * 2 + 2);
        if let Some(first) = self.first {
            points.push(first);
        }
        for bucket in self.buckets.into_iter().flatten() {
            if bucket.min.x <= bucket.max.x {
                points.push(bucket.min);
                points.push(bucket.max);
            } else {
                points.push(bucket.max);
                points.push(bucket.min);
            }
        }
        if let Some(last) = self.last {
            points.push(last);
        }
        points.dedup_by(|a, b| a.x == b.x && a.y == b.y);
        points
    }
}

impl UiClientApp {
    pub(crate) fn open_can_replay(&mut self) {
        self.can_replay.open = true;
        if self.can_replay.start_ts_input.trim().is_empty()
            || self.can_replay.end_ts_input.trim().is_empty()
        {
            let end_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            let start_ms = end_ms.saturating_sub(CAN_REPLAY_DEFAULT_WINDOW_MS);
            self.can_replay.start_ts_input = format_datetime_input(start_ms);
            self.can_replay.end_ts_input = format_datetime_input(end_ms);
        }
    }

    pub(crate) fn request_can_replay_load(&mut self) {
        if self.can_replay.loading {
            self.can_replay.status = "正在加载，请稍候".to_string();
            return;
        }

        let start_ts_ms = match parse_time_input(&self.can_replay.start_ts_input) {
            Some(v) => v,
            None => {
                self.can_replay.status = "开始时间请输入 ts_ms 或 YYYY-MM-DD HH:MM:SS".to_string();
                return;
            }
        };
        let end_ts_ms = match parse_time_input(&self.can_replay.end_ts_input) {
            Some(v) => v,
            None => {
                self.can_replay.status = "结束时间请输入 ts_ms 或 YYYY-MM-DD HH:MM:SS".to_string();
                return;
            }
        };
        if end_ts_ms <= start_ts_ms {
            self.can_replay.status = "结束时间必须大于开始时间".to_string();
            return;
        }

        self.can_replay.loading = true;
        self.can_replay.load_progress_current = 0;
        self.can_replay.load_progress_total = None;
        let dsn = self.can_replay.pg_dsn.clone();
        let mode = self.can_replay.mode;
        let downsampling_enabled = self.can_replay.downsampling_enabled;
        self.can_replay.status = self.can_replay.mode.load_status().to_string();
        let tx = self.ui_tx.clone();
        thread::spawn(move || {
            let progress_tx = tx.clone();
            let result = (|| -> Result<CanReplayData, String> {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|err| err.to_string())?;
                rt.block_on(async move {
                    let (client, connection) = tokio_postgres::connect(&dsn, NoTls)
                        .await
                        .map_err(|err| format!("connect postgres failed: {err}; dsn={dsn}"))?;
                    tokio::spawn(async move {
                        let _ = connection.await;
                    });

                    let axes = mode.axis_filters();
                    let count_row = client
                        .query_one(
                            "SELECT
                               (SELECT COUNT(*)
                                  FROM telemetry_samples
                                 WHERE device_id LIKE 'can://%'
                                   AND ts_ms >= $1
                                   AND ts_ms <= $2
                                   AND axis IN ($3, $4, $5, $6, $7)),
                               (SELECT COUNT(*)
                                  FROM alarm_events
                                 WHERE ts_ms >= $1
                                   AND ts_ms <= $2
                                   AND (alarm_id LIKE 'can_%' OR alarm_id LIKE 'sent_%'))",
                            &[
                                &start_ts_ms as &(dyn tokio_postgres::types::ToSql + Sync),
                                &end_ts_ms,
                                &axes[0],
                                &axes[1],
                                &axes[2],
                                &axes[3],
                                &axes[4],
                            ],
                        )
                        .await
                        .map_err(|err| format!("count replay rows failed: {err}"))?;
                    let telemetry_count: i64 = count_row.get(0);
                    let alarm_count: i64 = count_row.get(1);
                    let total_rows = u64::try_from(telemetry_count.max(0))
                        .unwrap_or(0)
                        .saturating_add(u64::try_from(alarm_count.max(0)).unwrap_or(0));
                    let _ = progress_tx.try_send(UiMsg::CanReplayProgress(mode, 0, total_rows));

                    let rows = client
                        .query_raw(
                            "SELECT ts_ms, axis, value
                             FROM telemetry_samples
                             WHERE device_id LIKE 'can://%'
                               AND ts_ms >= $1
                               AND ts_ms <= $2
                               AND axis IN ($3, $4, $5, $6, $7)
                             ORDER BY ts_ms ASC, axis ASC",
                            [
                                &start_ts_ms as &(dyn tokio_postgres::types::ToSql + Sync),
                                &end_ts_ms,
                                &axes[0],
                                &axes[1],
                                &axes[2],
                                &axes[3],
                                &axes[4],
                            ],
                        )
                        .await
                        .map_err(|err| err.to_string())?;

                    pin_mut!(rows);

                    let mut samplers = std::array::from_fn::<_, 5, _>(|_| {
                        ReplaySeriesSampler::new(
                            start_ts_ms,
                            end_ts_ms,
                            CAN_REPLAY_MAX_POINTS_PER_SERIES,
                            downsampling_enabled,
                        )
                    });
                    let mut raw_point_count = 0usize;
                    let mut processed_rows = 0u64;
                    while let Some(row) = rows
                        .try_next()
                        .await
                        .map_err(|err| format!("stream replay rows failed: {err}"))?
                    {
                        let ts_ms: i64 = row.get(0);
                        let axis: String = row.get(1);
                        let value: f64 = row.get(2);
                        if let Some(axis_index) =
                            axes.iter().position(|candidate| axis == *candidate)
                        {
                            samplers[axis_index].push(ts_ms, value);
                            raw_point_count = raw_point_count.saturating_add(1);
                        }
                        processed_rows = processed_rows.saturating_add(1);
                        if processed_rows % CAN_REPLAY_PROGRESS_REPORT_INTERVAL == 0 {
                            let _ = progress_tx.try_send(UiMsg::CanReplayProgress(
                                mode,
                                processed_rows,
                                total_rows,
                            ));
                        }
                    }

                    let [x_sampler, y_sampler, z_sampler, u_sampler, v_sampler] = samplers;
                    let mut data = CanReplayData {
                        x_points: x_sampler.finish(),
                        y_points: y_sampler.finish(),
                        z_points: z_sampler.finish(),
                        u_points: u_sampler.finish(),
                        v_points: v_sampler.finish(),
                        min_ts_ms: start_ts_ms,
                        max_ts_ms: end_ts_ms,
                        raw_point_count,
                        ..CanReplayData::default()
                    };

                    let alarm_rows = client
                        .query_raw(
                            "SELECT ts_ms, alarm_id, level, message, cleared
                             FROM alarm_events
                             WHERE ts_ms >= $1
                               AND ts_ms <= $2
                               AND (alarm_id LIKE 'can_%' OR alarm_id LIKE 'sent_%')
                             ORDER BY ts_ms ASC, id ASC",
                            [
                                &start_ts_ms as &(dyn tokio_postgres::types::ToSql + Sync),
                                &end_ts_ms,
                            ],
                        )
                        .await
                        .map_err(|err| err.to_string())?;

                    pin_mut!(alarm_rows);
                    while let Some(row) = alarm_rows
                        .try_next()
                        .await
                        .map_err(|err| format!("stream replay alarms failed: {err}"))?
                    {
                        let ts_ms: i64 = row.get(0);
                        let alarm_id: String = row.get(1);
                        let level: String = row.get(2);
                        let message: String = row.get(3);
                        let cleared: bool = row.get(4);
                        let x_sec = (ts_ms - start_ts_ms) as f64 / 1000.0;
                        match mode {
                            ReplayMode::Can3Axis => {
                                let target = if alarm_id.starts_with("can_x_") {
                                    Some((&data.x_points, &mut data.x_alarm_points))
                                } else if alarm_id.starts_with("can_y_") {
                                    Some((&data.y_points, &mut data.y_alarm_points))
                                } else if alarm_id.starts_with("can_z_") {
                                    Some((&data.z_points, &mut data.z_alarm_points))
                                } else {
                                    None
                                };
                                if let Some((series, alarms)) = target
                                    && alarms.len() < CAN_REPLAY_MAX_ALARM_POINTS_PER_SERIES
                                    && let Some(point) = nearest_plot_point(series, x_sec)
                                {
                                    alarms.push(AlarmPlotPoint {
                                        point: [x_sec, point[1]],
                                        level,
                                        cleared,
                                    });
                                }
                            }
                            ReplayMode::Sent => {
                                let target = if alarm_id == "sent_torque_jump_t1" {
                                    Some((&data.y_points, &mut data.y_alarm_points))
                                } else if alarm_id == "sent_angle_jump_t1"
                                    || message.contains("t1=1")
                                {
                                    Some((&data.x_points, &mut data.x_alarm_points))
                                } else if alarm_id == "sent_torque_jump_t2" {
                                    Some((&data.u_points, &mut data.u_alarm_points))
                                } else if alarm_id == "sent_angle_jump_t2"
                                    || message.contains("t2=1")
                                {
                                    Some((&data.z_points, &mut data.z_alarm_points))
                                } else if alarm_id == "sent_angle_jump_s" || message.contains("s=1")
                                {
                                    Some((&data.v_points, &mut data.v_alarm_points))
                                } else {
                                    None
                                };
                                if let Some((series, alarms)) = target
                                    && alarms.len() < CAN_REPLAY_MAX_ALARM_POINTS_PER_SERIES
                                    && let Some(point) = nearest_plot_point(series, x_sec)
                                {
                                    alarms.push(AlarmPlotPoint {
                                        point: [x_sec, point[1]],
                                        level,
                                        cleared,
                                    });
                                }
                            }
                        }
                        processed_rows = processed_rows.saturating_add(1);
                        if processed_rows % CAN_REPLAY_PROGRESS_REPORT_INTERVAL == 0 {
                            let _ = progress_tx.try_send(UiMsg::CanReplayProgress(
                                mode,
                                processed_rows,
                                total_rows,
                            ));
                        }
                    }

                    let _ = progress_tx
                        .try_send(UiMsg::CanReplayProgress(mode, total_rows, total_rows));

                    Ok(data)
                })
            })();

            let _ = tx.send(UiMsg::CanReplayLoaded(mode, result));
        });
    }

    pub(crate) fn request_can_replay_export(&mut self) {
        if self.can_replay.exporting {
            self.can_replay.status = "正在导出，请稍候...".to_string();
            return;
        }

        let start_ts_ms = match parse_time_input(&self.can_replay.start_ts_input) {
            Some(v) => v,
            None => {
                self.can_replay.status = "开始时间请输入 ts_ms 或 YYYY-MM-DD HH:MM:SS".to_string();
                return;
            }
        };
        let end_ts_ms = match parse_time_input(&self.can_replay.end_ts_input) {
            Some(v) => v,
            None => {
                self.can_replay.status = "结束时间请输入 ts_ms 或 YYYY-MM-DD HH:MM:SS".to_string();
                return;
            }
        };
        if end_ts_ms <= start_ts_ms {
            self.can_replay.status = "结束时间必须大于开始时间".to_string();
            return;
        }
        let has_selected_series = self.can_replay.show_x
            || self.can_replay.show_y
            || self.can_replay.show_z
            || (self.can_replay.mode == ReplayMode::Sent
                && (self.can_replay.show_u || self.can_replay.show_v));
        if !has_selected_series {
            self.can_replay.status = "至少勾选一个轴后再导出".to_string();
            return;
        }

        self.can_replay.exporting = true;
        self.can_replay.status = "正在流式导出 txt...".to_string();
        let dsn = self.can_replay.pg_dsn.clone();
        let mode = self.can_replay.mode;
        let tx = self.ui_tx.clone();
        let show_x = self.can_replay.show_x;
        let show_y = self.can_replay.show_y;
        let show_z = self.can_replay.show_z;
        let show_u = mode == ReplayMode::Sent && self.can_replay.show_u;
        let show_v = mode == ReplayMode::Sent && self.can_replay.show_v;
        thread::spawn(move || {
            let result = (|| -> Result<String, String> {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|err| err.to_string())?;
                rt.block_on(async move {
                    let (client, connection) = tokio_postgres::connect(&dsn, NoTls)
                        .await
                        .map_err(|err| err.to_string())?;
                    tokio::spawn(async move {
                        let _ = connection.await;
                    });

                    let now_ts_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0);
                    fs::create_dir_all(CAN_EXPORT_DIR).map_err(|err| {
                        format!("create export dir failed: {err}; dir={CAN_EXPORT_DIR}")
                    })?;
                    let export_path = std::path::PathBuf::from(CAN_EXPORT_DIR).join(
                        default_can_export_filename(start_ts_ms, end_ts_ms, now_ts_ms),
                    );
                    let file = fs::File::create(&export_path).map_err(|err| {
                        format!(
                            "create export file failed: {err}; path={}",
                            export_path.display()
                        )
                    })?;
                    let mut writer = std::io::BufWriter::new(file);

                    writeln!(writer, "{}", mode.export_header())
                        .map_err(|err| format!("write export header failed: {err}"))?;
                    writeln!(writer, "# start_ts_ms={start_ts_ms}")
                        .map_err(|err| format!("write start_ts_ms failed: {err}"))?;
                    writeln!(writer, "# end_ts_ms={end_ts_ms}")
                        .map_err(|err| format!("write end_ts_ms failed: {err}"))?;
                    writeln!(writer, "ts_ms\tdevice_id\taxis\tvalue\trequest_id")
                        .map_err(|err| format!("write export columns failed: {err}"))?;

                    let axes = mode.axis_filters();
                    let sql = "SELECT ts_ms, device_id, axis, value, request_id
                               FROM telemetry_samples
                               WHERE device_id LIKE 'can://%'
                                 AND ts_ms >= $1
                                 AND ts_ms <= $2
                                 AND (($3 AND axis = $8)
                                   OR ($4 AND axis = $9)
                                   OR ($5 AND axis = $10)
                                   OR ($6 AND axis = $11)
                                   OR ($7 AND axis = $12))
                               ORDER BY ts_ms ASC, axis ASC";

                    let params: [&(dyn tokio_postgres::types::ToSql + Sync); 12] = [
                        &start_ts_ms,
                        &end_ts_ms,
                        &show_x,
                        &show_y,
                        &show_z,
                        &show_u,
                        &show_v,
                        &axes[0],
                        &axes[1],
                        &axes[2],
                        &axes[3],
                        &axes[4],
                    ];
                    let rows = client
                        .query_raw(sql, params)
                        .await
                        .map_err(|err| format!("query export rows failed: {err}"))?;
                    pin_mut!(rows);

                    while let Some(row) = rows
                        .try_next()
                        .await
                        .map_err(|err| format!("stream export rows failed: {err}"))?
                    {
                        let ts_ms: i64 = row.get(0);
                        let device_id: String = row.get(1);
                        let axis: String = row.get(2);
                        let value: f64 = row.get(3);
                        let request_id: i64 = row.get(4);
                        writeln!(
                            writer,
                            "{ts_ms}\t{device_id}\t{axis}\t{value:.6}\t{request_id}"
                        )
                        .map_err(|err| format!("write export row failed: {err}"))?;
                    }
                    writer
                        .flush()
                        .map_err(|err| format!("flush export file failed: {err}"))?;

                    Ok(export_path.display().to_string())
                })
            })();

            let _ = tx.send(UiMsg::CanReplayExported(result));
        });
    }

    pub(crate) fn format_ts_ms(ts_ms: i64) -> String {
        let total_seconds = ts_ms.div_euclid(1000) + DISPLAY_TZ_OFFSET_SECS;
        let seconds_of_day = total_seconds.rem_euclid(86_400);
        let hour = seconds_of_day / 3600;
        let minute = (seconds_of_day % 3600) / 60;
        let second = seconds_of_day % 60;
        format!("{hour:02}:{minute:02}:{second:02}")
    }

    pub(crate) fn draw_alarm_plot_points(
        plot_ui: &mut egui_plot::PlotUi<'_>,
        label: &str,
        points: &[AlarmPlotPoint],
    ) {
        let mut groups: Vec<(egui::Color32, bool, Vec<[f64; 2]>)> = Vec::new();
        for alarm in points {
            let color = Self::alarm_record_level_color(&alarm.level);
            if let Some((_, _, grouped_points)) =
                groups.iter_mut().find(|(group_color, cleared, _)| {
                    *group_color == color && *cleared == alarm.cleared
                })
            {
                grouped_points.push(alarm.point);
            } else {
                groups.push((color, alarm.cleared, vec![alarm.point]));
            }
        }

        for (index, (color, cleared, grouped_points)) in groups.into_iter().enumerate() {
            let radius = if cleared { 4.0 } else { 6.0 };
            plot_ui.points(
                Points::new(
                    format!("{label} alarm group {index}"),
                    PlotPoints::new(grouped_points),
                )
                .color(color)
                .radius(radius)
                .filled(!cleared),
            );
        }
    }

    pub(crate) fn draw_can_replay_chart(
        &self,
        ui: &mut egui::Ui,
        data: &CanReplayData,
        height: f32,
    ) -> Option<((i64, i64), egui::Rect)> {
        let plot_response = Plot::new("can_replay_plot")
            .allow_zoom([true, true])
            .allow_scroll([true, true])
            .allow_drag([true, true])
            .height(height.max(260.0))
            .x_axis_formatter({
                let base_ts_ms = data.min_ts_ms;
                move |mark, _range| {
                    let ts_ms = base_ts_ms + (mark.value * 1000.0) as i64;
                    Self::format_ts_ms(ts_ms)
                }
            })
            .show(ui, |plot_ui| {
                let labels = self.can_replay.mode.series_labels();
                if self.can_replay.show_x {
                    plot_ui.line(
                        Line::new(labels[0], PlotPoints::from(data.x_points.as_slice()))
                            .color(egui::Color32::from_rgb(239, 83, 80)),
                    );
                    if self.can_replay.show_alarm_points {
                        Self::draw_alarm_plot_points(plot_ui, labels[0], &data.x_alarm_points);
                    }
                }
                if self.can_replay.show_y {
                    plot_ui.line(
                        Line::new(labels[1], PlotPoints::from(data.y_points.as_slice()))
                            .color(egui::Color32::from_rgb(66, 165, 245)),
                    );
                    if self.can_replay.show_alarm_points {
                        Self::draw_alarm_plot_points(plot_ui, labels[1], &data.y_alarm_points);
                    }
                }
                if self.can_replay.show_z {
                    plot_ui.line(
                        Line::new(labels[2], PlotPoints::from(data.z_points.as_slice()))
                            .color(egui::Color32::from_rgb(102, 187, 106)),
                    );
                    if self.can_replay.show_alarm_points {
                        Self::draw_alarm_plot_points(plot_ui, labels[2], &data.z_alarm_points);
                    }
                }
                if self.can_replay.mode == ReplayMode::Sent && self.can_replay.show_u {
                    plot_ui.line(
                        Line::new(labels[3], PlotPoints::from(data.u_points.as_slice()))
                            .color(egui::Color32::from_rgb(255, 167, 38)),
                    );
                    if self.can_replay.show_alarm_points {
                        Self::draw_alarm_plot_points(plot_ui, labels[3], &data.u_alarm_points);
                    }
                }
                if self.can_replay.mode == ReplayMode::Sent && self.can_replay.show_v {
                    plot_ui.line(
                        Line::new(labels[4], PlotPoints::from(data.v_points.as_slice()))
                            .color(egui::Color32::from_rgb(171, 71, 188)),
                    );
                    if self.can_replay.show_alarm_points {
                        Self::draw_alarm_plot_points(plot_ui, labels[4], &data.v_alarm_points);
                    }
                }
                plot_ui.plot_bounds()
            });

        let bounds = plot_response.inner;
        let x_range = bounds.range_x();
        let start_ms = data.min_ts_ms + (*x_range.start() * 1000.0) as i64;
        let end_ms = data.min_ts_ms + (*x_range.end() * 1000.0) as i64;
        Some(((start_ms, end_ms), plot_response.response.rect))
    }

    pub(crate) fn draw_can_replay_window(&mut self, ctx: &egui::Context) {
        if !self.can_replay.open {
            return;
        }

        let mut open = self.can_replay.open;
        let mut replay_plot_rect = None;
        egui::Window::new("CAN 回放")
            .open(&mut open)
            .default_size(egui::vec2(1080.0, 640.0))
            .min_width(860.0)
            .min_height(520.0)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("模式");
                    let previous_mode = self.can_replay.mode;
                    ui.add_enabled_ui(!self.can_replay.loading, |ui| {
                        ui.selectable_value(&mut self.can_replay.mode, ReplayMode::Can3Axis, "3轴");
                        ui.selectable_value(&mut self.can_replay.mode, ReplayMode::Sent, "SENT");
                    });
                    if self.can_replay.mode != previous_mode {
                        self.can_replay.data = None;
                        self.can_replay.plot_rect = None;
                        self.can_replay.status = self.can_replay.mode.empty_status().to_string();
                    }
                    ui.label("开始时间");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.can_replay.start_ts_input)
                            .desired_width(220.0),
                    );
                    ui.label("结束时间");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.can_replay.end_ts_input)
                            .desired_width(220.0),
                    );
                    if ui
                        .add_enabled(!self.can_replay.loading, egui::Button::new("加载回放"))
                        .clicked()
                    {
                        self.request_can_replay_load();
                    }
                    if ui
                        .add_enabled(
                            !self.can_replay.loading && !self.can_replay.exporting,
                            egui::Button::new("导出TXT"),
                        )
                        .clicked()
                    {
                        self.request_can_replay_export();
                    }
                    if ui.button("最近5分钟").clicked() {
                        let end_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0);
                        self.can_replay.start_ts_input = format_datetime_input(
                            end_ms.saturating_sub(CAN_REPLAY_DEFAULT_WINDOW_MS),
                        );
                        self.can_replay.end_ts_input = format_datetime_input(end_ms);
                    }
                });

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let labels = self.can_replay.mode.series_labels();
                    ui.label("轴显示");
                    ui.toggle_value(&mut self.can_replay.show_x, labels[0]);
                    ui.toggle_value(&mut self.can_replay.show_y, labels[1]);
                    ui.toggle_value(&mut self.can_replay.show_z, labels[2]);
                    if self.can_replay.mode == ReplayMode::Sent {
                        ui.toggle_value(&mut self.can_replay.show_u, labels[3]);
                        ui.toggle_value(&mut self.can_replay.show_v, labels[4]);
                    }
                    ui.separator();
                    ui.toggle_value(&mut self.can_replay.show_alarm_points, "报警点");
                    ui.separator();
                    let changed = ui
                        .add_enabled_ui(!self.can_replay.loading, |ui| {
                            ui.checkbox(
                                &mut self.can_replay.downsampling_enabled,
                                format!(
                                    "回放降采样（每路最多 {} 点）",
                                    CAN_REPLAY_MAX_POINTS_PER_SERIES
                                ),
                            )
                        })
                        .inner
                        .changed();
                    if changed {
                        self.can_replay.data = None;
                        self.can_replay.plot_rect = None;
                        self.can_replay.status = "降采样设置已更改，请重新加载回放".to_string();
                    }
                });

                if !self.can_replay.downsampling_enabled {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        "回放降采样已关闭：将加载全部原始点，较大时间范围可能占用大量内存",
                    );
                }

                if self.can_replay.loading {
                    match self.can_replay.load_progress_total {
                        Some(total) if total > 0 => {
                            let current = self.can_replay.load_progress_current.min(total);
                            let progress = current as f32 / total as f32;
                            ui.add(
                                egui::ProgressBar::new(progress)
                                    .show_percentage()
                                    .text(format!("已处理 {current} / {total} 条记录")),
                            );
                        }
                        Some(_) => {
                            ui.add(
                                egui::ProgressBar::new(1.0)
                                    .show_percentage()
                                    .text("没有匹配的回放记录"),
                            );
                        }
                        None => {
                            ui.add(
                                egui::ProgressBar::new(0.0)
                                    .animate(true)
                                    .text("正在统计回放记录..."),
                            );
                        }
                    }
                }

                ui.label(&self.can_replay.status);
                ui.small("支持 ts_ms，或 YYYY-MM-DD HH:MM:SS");
                ui.add_space(6.0);

                let available_height = (ui.available_height() - 56.0).max(240.0);
                if let Some(data) = self.can_replay.data.as_ref() {
                    let chart_result = self.draw_can_replay_chart(ui, data, available_height);
                    ui.horizontal(|ui| {
                        ui.label(format!("ts_ms: {}", data.min_ts_ms));
                        ui.add_space((ui.available_width() - 160.0).max(0.0));
                        ui.label(format!("ts_ms: {}", data.max_ts_ms));
                    });
                    if let Some(((visible_start_ms, visible_end_ms), plot_rect)) = chart_result {
                        replay_plot_rect = Some(plot_rect);
                        ui.label(format!(
                            "当前视图: {} -> {}",
                            visible_start_ms, visible_end_ms
                        ));
                    }
                } else {
                    ui.group(|ui| {
                        ui.set_min_height(available_height);
                        ui.vertical_centered(|ui| {
                            ui.add_space(available_height * 0.35);
                            ui.heading("CAN 回放");
                            ui.label("输入开始/结束时间后点击“加载回放”");
                        });
                    });
                }
            });
        self.can_replay.open = open;
        self.can_replay.plot_rect = replay_plot_rect;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_sampler_bounds_output_and_keeps_extrema() {
        let max_points = 100;
        let mut sampler = ReplaySeriesSampler::new(0, 10_000, max_points, true);
        for ts_ms in 0..10_000 {
            let value = match ts_ms {
                3_211 => 500.0,
                7_654 => -400.0,
                _ => (ts_ms % 20) as f64,
            };
            sampler.push(ts_ms, value);
        }

        let points = sampler.finish();

        assert!(points.len() <= max_points);
        assert!(points.iter().any(|point| point.y == 500.0));
        assert!(points.iter().any(|point| point.y == -400.0));
        assert!(points.windows(2).all(|pair| pair[0].x <= pair[1].x));
    }

    #[test]
    fn replay_sampler_leaves_small_series_unchanged() {
        // Even if the selected replay window is much wider than the actual
        // burst, a series below the cap must not be collapsed into one bucket.
        let mut sampler = ReplaySeriesSampler::new(1_000, 1_000_000, 20, true);
        sampler.push(1_000, 1.0);
        sampler.push(1_001, 2.0);
        sampler.push(1_002, 3.0);

        let points = sampler.finish();

        assert_eq!(points.len(), 3);
        assert_eq!((points[0].x, points[0].y), (0.0, 1.0));
        assert_eq!((points[1].x, points[1].y), (0.001, 2.0));
        assert_eq!((points[2].x, points[2].y), (0.002, 3.0));
    }

    #[test]
    fn replay_sampler_keeps_all_points_when_disabled() {
        let mut sampler = ReplaySeriesSampler::new(0, 10_000, 20, false);
        for ts_ms in 0..1_000 {
            sampler.push(ts_ms, ts_ms as f64);
        }

        let points = sampler.finish();

        assert_eq!(points.len(), 1_000);
        assert_eq!((points[321].x, points[321].y), (0.321, 321.0));
    }
}
