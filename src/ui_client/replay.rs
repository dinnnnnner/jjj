use crate::*;
use egui_plot::{Line, Plot, PlotPoints, Points};
use futures_util::{TryStreamExt, pin_mut};
use std::fs;
use std::io::Write;
use std::thread;
use tokio_postgres::NoTls;

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
        let dsn = self.can_replay.pg_dsn.clone();
        let mode = self.can_replay.mode;
        self.can_replay.status = self.can_replay.mode.load_status().to_string();
        let tx = self.ui_tx.clone();
        thread::spawn(move || {
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
                    let rows = client
                        .query(
                            "SELECT ts_ms, axis, value
                             FROM telemetry_samples
                             WHERE device_id LIKE 'can://%'
                               AND ts_ms >= $1
                               AND ts_ms <= $2
                               AND axis IN ($3, $4, $5, $6, $7)
                             ORDER BY ts_ms ASC, axis ASC",
                            &[
                                &start_ts_ms,
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

                    let mut data = CanReplayData {
                        min_ts_ms: start_ts_ms,
                        max_ts_ms: end_ts_ms,
                        ..CanReplayData::default()
                    };
                    for row in rows {
                        let ts_ms: i64 = row.get(0);
                        let axis: String = row.get(1);
                        let value: f64 = row.get(2);
                        let x_sec = (ts_ms - start_ts_ms) as f64 / 1000.0;
                        match axis.as_str() {
                            axis_name if axis_name == axes[0] => data.x_points.push([x_sec, value]),
                            axis_name if axis_name == axes[1] => data.y_points.push([x_sec, value]),
                            axis_name if axis_name == axes[2] => data.z_points.push([x_sec, value]),
                            axis_name if axis_name == axes[3] => data.u_points.push([x_sec, value]),
                            axis_name if axis_name == axes[4] => data.v_points.push([x_sec, value]),
                            _ => {}
                        }
                    }

                    let alarm_rows = client
                        .query(
                            "SELECT ts_ms, alarm_id, level, message, cleared
                             FROM alarm_events
                             WHERE ts_ms >= $1
                               AND ts_ms <= $2
                               AND (alarm_id LIKE 'can_%' OR alarm_id LIKE 'sent_%')
                             ORDER BY ts_ms ASC, id ASC",
                            &[&start_ts_ms, &end_ts_ms],
                        )
                        .await
                        .map_err(|err| err.to_string())?;

                    for row in alarm_rows {
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
                                if let Some((series, alarms)) = target {
                                    if let Some(point) = nearest_plot_point(series, x_sec) {
                                        alarms.push(AlarmPlotPoint {
                                            point: [x_sec, point[1]],
                                            level,
                                            cleared,
                                        });
                                    }
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
                                if let Some((series, alarms)) = target {
                                    if let Some(point) = nearest_plot_point(series, x_sec) {
                                        alarms.push(AlarmPlotPoint {
                                            point: [x_sec, point[1]],
                                            level,
                                            cleared,
                                        });
                                    }
                                }
                            }
                        }
                    }

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
        for (idx, alarm) in points.iter().enumerate() {
            let color = Self::alarm_record_level_color(&alarm.level);
            let radius = if alarm.cleared { 4.0 } else { 6.0 };
            plot_ui.points(
                Points::new(
                    format!("{label} alarm {idx}"),
                    PlotPoints::new(vec![alarm.point]),
                )
                .color(color)
                .radius(radius)
                .filled(!alarm.cleared),
            );
        }
    }

    pub(crate) fn draw_can_replay_chart(
        &mut self,
        ui: &mut egui::Ui,
        data: &CanReplayData,
        height: f32,
    ) -> Option<(i64, i64)> {
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
                        Line::new(labels[0], PlotPoints::new(data.x_points.clone()))
                            .color(egui::Color32::from_rgb(239, 83, 80)),
                    );
                    if self.can_replay.show_alarm_points {
                        Self::draw_alarm_plot_points(plot_ui, labels[0], &data.x_alarm_points);
                    }
                }
                if self.can_replay.show_y {
                    plot_ui.line(
                        Line::new(labels[1], PlotPoints::new(data.y_points.clone()))
                            .color(egui::Color32::from_rgb(66, 165, 245)),
                    );
                    if self.can_replay.show_alarm_points {
                        Self::draw_alarm_plot_points(plot_ui, labels[1], &data.y_alarm_points);
                    }
                }
                if self.can_replay.show_z {
                    plot_ui.line(
                        Line::new(labels[2], PlotPoints::new(data.z_points.clone()))
                            .color(egui::Color32::from_rgb(102, 187, 106)),
                    );
                    if self.can_replay.show_alarm_points {
                        Self::draw_alarm_plot_points(plot_ui, labels[2], &data.z_alarm_points);
                    }
                }
                if self.can_replay.mode == ReplayMode::Sent && self.can_replay.show_u {
                    plot_ui.line(
                        Line::new(labels[3], PlotPoints::new(data.u_points.clone()))
                            .color(egui::Color32::from_rgb(255, 167, 38)),
                    );
                    if self.can_replay.show_alarm_points {
                        Self::draw_alarm_plot_points(plot_ui, labels[3], &data.u_alarm_points);
                    }
                }
                if self.can_replay.mode == ReplayMode::Sent && self.can_replay.show_v {
                    plot_ui.line(
                        Line::new(labels[4], PlotPoints::new(data.v_points.clone()))
                            .color(egui::Color32::from_rgb(171, 71, 188)),
                    );
                    if self.can_replay.show_alarm_points {
                        Self::draw_alarm_plot_points(plot_ui, labels[4], &data.v_alarm_points);
                    }
                }
                plot_ui.plot_bounds()
            });

        self.can_replay.plot_rect = Some(plot_response.response.rect);
        let bounds = plot_response.inner;
        let x_range = bounds.range_x();
        let start_ms = data.min_ts_ms + (*x_range.start() * 1000.0) as i64;
        let end_ms = data.min_ts_ms + (*x_range.end() * 1000.0) as i64;
        Some((start_ms, end_ms))
    }

    pub(crate) fn draw_can_replay_window(&mut self, ctx: &egui::Context) {
        if !self.can_replay.open {
            return;
        }

        let mut open = self.can_replay.open;
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
                });

                ui.label(&self.can_replay.status);
                ui.small("支持 ts_ms，或 YYYY-MM-DD HH:MM:SS");
                ui.add_space(6.0);

                let available_height = (ui.available_height() - 56.0).max(240.0);
                if let Some(data) = self.can_replay.data.clone() {
                    let visible_range = self.draw_can_replay_chart(ui, &data, available_height);
                    ui.horizontal(|ui| {
                        ui.label(format!("ts_ms: {}", data.min_ts_ms));
                        ui.add_space((ui.available_width() - 160.0).max(0.0));
                        ui.label(format!("ts_ms: {}", data.max_ts_ms));
                    });
                    if let Some((visible_start_ms, visible_end_ms)) = visible_range {
                        ui.label(format!(
                            "当前视图: {} -> {}",
                            visible_start_ms, visible_end_ms
                        ));
                    }
                } else {
                    self.can_replay.plot_rect = None;
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
    }
}
