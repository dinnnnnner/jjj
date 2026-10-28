use crate::*;
use std::thread;
use tokio_postgres::NoTls;

impl UiClientApp {
    pub(crate) fn open_alarm_records(&mut self) {
        self.alarm_records.open = true;
        if self.alarm_records.start_ts_input.trim().is_empty()
            || self.alarm_records.end_ts_input.trim().is_empty()
        {
            let end_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            let start_ms = end_ms.saturating_sub(ALARM_RECORD_DEFAULT_WINDOW_MS);
            self.alarm_records.start_ts_input = format_datetime_input(start_ms);
            self.alarm_records.end_ts_input = format_datetime_input(end_ms);
        }
    }

    pub(crate) fn open_can_replay_from_alarm_records(&mut self) {
        let Some(data) = self.alarm_records.data.as_ref() else {
            self.alarm_records.status = "请先加载报警记录".to_string();
            return;
        };
        let Some(first_alarm_ts) = data.records.iter().map(|row| row.ts_ms).min() else {
            self.alarm_records.status = "当前页没有报警记录，无法打开时间分布".to_string();
            return;
        };
        let last_alarm_ts = data
            .records
            .iter()
            .map(|row| row.ts_ms)
            .max()
            .unwrap_or(first_alarm_ts);
        let replay_start_ts = first_alarm_ts.saturating_sub(ALARM_REPLAY_CONTEXT_MS);
        let replay_end_ts = last_alarm_ts.saturating_add(ALARM_REPLAY_CONTEXT_MS);

        self.can_replay.open = true;
        self.can_replay.start_ts_input = format_datetime_input(replay_start_ts);
        self.can_replay.end_ts_input = format_datetime_input(replay_end_ts);
        let has_can_group = self.alarm_records.show_can_x
            || self.alarm_records.show_can_y
            || self.alarm_records.show_can_z;
        self.can_replay.mode = if has_can_group {
            ReplayMode::Can3Axis
        } else {
            ReplayMode::Sent
        };
        self.can_replay.show_x = match self.can_replay.mode {
            ReplayMode::Can3Axis => self.alarm_records.show_can_x,
            ReplayMode::Sent => self.alarm_records.show_sent_t1,
        };
        self.can_replay.show_y = match self.can_replay.mode {
            ReplayMode::Can3Axis => self.alarm_records.show_can_y,
            ReplayMode::Sent => self.alarm_records.show_sent_t1,
        };
        self.can_replay.show_z = match self.can_replay.mode {
            ReplayMode::Can3Axis => self.alarm_records.show_can_z,
            ReplayMode::Sent => self.alarm_records.show_sent_t2,
        };
        self.can_replay.show_u =
            self.can_replay.mode == ReplayMode::Sent && self.alarm_records.show_sent_t2;
        self.can_replay.show_v =
            self.can_replay.mode == ReplayMode::Sent && self.alarm_records.show_sent_s;
        self.can_replay.data = None;
        self.can_replay.plot_rect = None;
        self.request_can_replay_load();
    }

    pub(crate) fn request_alarm_records_load(&mut self) {
        self.request_alarm_records_page(0);
    }

    pub(crate) fn request_alarm_records_page(&mut self, page_index: i64) {
        if self.alarm_records.loading {
            self.alarm_records.status = "正在加载，请稍候".to_string();
            return;
        }

        let start_ts_ms = match parse_time_input(&self.alarm_records.start_ts_input) {
            Some(v) => v,
            None => {
                self.alarm_records.status = "开始时间无效".to_string();
                return;
            }
        };
        let end_ts_ms = match parse_time_input(&self.alarm_records.end_ts_input) {
            Some(v) => v,
            None => {
                self.alarm_records.status = "结束时间无效".to_string();
                return;
            }
        };
        if end_ts_ms <= start_ts_ms {
            self.alarm_records.status = "结束时间必须大于开始时间".to_string();
            return;
        }

        let has_selected_group = self.alarm_records.show_can_x
            || self.alarm_records.show_can_y
            || self.alarm_records.show_can_z
            || self.alarm_records.show_can_timeout
            || self.alarm_records.show_sent_t1
            || self.alarm_records.show_sent_t2
            || self.alarm_records.show_sent_s;
        if !has_selected_group {
            self.alarm_records.status = "至少选择一个分组".to_string();
            return;
        }

        let page_index = page_index.max(0);
        let offset = page_index * ALARM_RECORD_PAGE_SIZE;
        let fetch_limit = ALARM_RECORD_PAGE_SIZE + 1;
        self.alarm_records.loading = true;
        self.alarm_records.status = format!("正在加载第 {} 页报警记录...", page_index + 1);

        let dsn = self.alarm_records.pg_dsn.clone();
        let tx = self.ui_tx.clone();
        let show_can_x = self.alarm_records.show_can_x;
        let show_can_y = self.alarm_records.show_can_y;
        let show_can_z = self.alarm_records.show_can_z;
        let show_can_timeout = self.alarm_records.show_can_timeout;
        let show_sent_t1 = self.alarm_records.show_sent_t1;
        let show_sent_t2 = self.alarm_records.show_sent_t2;
        let show_sent_s = self.alarm_records.show_sent_s;
        thread::spawn(move || {
            let result = (|| -> Result<AlarmRecordData, String> {
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

                    let total_count: i64 = client
                        .query_one(
                            "SELECT COUNT(*)
                             FROM alarm_events
                             WHERE ts_ms >= $1
                               AND ts_ms <= $2
                               AND (($3 AND alarm_id IN ('can_x_h', 'can_x_l'))
                                 OR ($4 AND alarm_id IN ('can_y_h', 'can_y_l'))
                                 OR ($5 AND alarm_id IN ('can_z_h', 'can_z_l'))
                                 OR ($6 AND alarm_id LIKE 'can_signal_timeout_%')
                                 OR ($7 AND (alarm_id IN ('sent_torque_jump_t1', 'sent_angle_jump_t1') OR message LIKE '%t1=1%'))
                                 OR ($8 AND (alarm_id IN ('sent_torque_jump_t2', 'sent_angle_jump_t2') OR message LIKE '%t2=1%'))
                                 OR ($9 AND (alarm_id = 'sent_angle_jump_s' OR message LIKE '%s=1%')))",
                            &[
                                &start_ts_ms,
                                &end_ts_ms,
                                &show_can_x,
                                &show_can_y,
                                &show_can_z,
                                &show_can_timeout,
                                &show_sent_t1,
                                &show_sent_t2,
                                &show_sent_s,
                            ],
                        )
                        .await
                        .map_err(|err| err.to_string())?
                        .get(0);

                    let rows = client
                        .query(
                            "SELECT ts_ms, device_id, alarm_id, level, message, cleared
                             FROM alarm_events
                             WHERE ts_ms >= $1
                               AND ts_ms <= $2
                               AND (($3 AND alarm_id IN ('can_x_h', 'can_x_l'))
                                 OR ($4 AND alarm_id IN ('can_y_h', 'can_y_l'))
                                 OR ($5 AND alarm_id IN ('can_z_h', 'can_z_l'))
                                 OR ($6 AND alarm_id LIKE 'can_signal_timeout_%')
                                 OR ($7 AND (alarm_id IN ('sent_torque_jump_t1', 'sent_angle_jump_t1') OR message LIKE '%t1=1%'))
                                 OR ($8 AND (alarm_id IN ('sent_torque_jump_t2', 'sent_angle_jump_t2') OR message LIKE '%t2=1%'))
                                 OR ($9 AND (alarm_id = 'sent_angle_jump_s' OR message LIKE '%s=1%')))
                             ORDER BY ts_ms ASC, id ASC
                             LIMIT $10 OFFSET $11",
                            &[
                                &start_ts_ms,
                                &end_ts_ms,
                                &show_can_x,
                                &show_can_y,
                                &show_can_z,
                                &show_can_timeout,
                                &show_sent_t1,
                                &show_sent_t2,
                                &show_sent_s,
                                &fetch_limit,
                                &offset,
                            ],
                        )
                        .await
                        .map_err(|err| err.to_string())?;

                    let has_next = rows.len() as i64 > ALARM_RECORD_PAGE_SIZE;
                    let records = rows
                        .into_iter()
                        .take(ALARM_RECORD_PAGE_SIZE as usize)
                        .map(|row| AlarmRecordRow {
                            ts_ms: row.get(0),
                            device_id: row.get(1),
                            alarm_id: row.get(2),
                            level: row.get(3),
                            message: row.get(4),
                            cleared: row.get(5),
                        })
                        .collect();

                    Ok(AlarmRecordData {
                        records,
                        page_index,
                        has_next,
                        total_count,
                    })
                })
            })();

            let _ = tx.send(UiMsg::AlarmRecordsLoaded(result));
        });
    }

    pub(crate) fn alarm_record_matches_can_axis(row: &AlarmRecordRow, axis: &str) -> bool {
        row.alarm_id.starts_with(&format!("can_{axis}_"))
    }

    pub(crate) fn alarm_record_matches_can_timeout(row: &AlarmRecordRow) -> bool {
        row.alarm_id.starts_with("can_signal_timeout_")
    }

    pub(crate) fn alarm_record_matches_sent_part(row: &AlarmRecordRow, part: &str) -> bool {
        match part {
            "t1" => row.alarm_id.ends_with("_t1") || row.message.contains("t1=1"),
            "t2" => row.alarm_id.ends_with("_t2") || row.message.contains("t2=1"),
            "s" => row.alarm_id.ends_with("_s") || row.message.contains("s=1"),
            _ => false,
        }
    }

    pub(crate) fn alarm_record_groups(row: &AlarmRecordRow) -> String {
        let mut groups = Vec::new();
        if Self::alarm_record_matches_can_axis(row, "x") {
            groups.push("CAN X");
        }
        if Self::alarm_record_matches_can_axis(row, "y") {
            groups.push("CAN Y");
        }
        if Self::alarm_record_matches_can_axis(row, "z") {
            groups.push("CAN Z");
        }
        if Self::alarm_record_matches_can_timeout(row) {
            groups.push("CAN Timeout");
        }
        if Self::alarm_record_matches_sent_part(row, "t1") {
            groups.push("SENT T1");
        }
        if Self::alarm_record_matches_sent_part(row, "t2") {
            groups.push("SENT T2");
        }
        if Self::alarm_record_matches_sent_part(row, "s") {
            groups.push("SENT S");
        }
        if groups.is_empty() {
            "Other".to_string()
        } else {
            groups.join(", ")
        }
    }

    pub(crate) fn alarm_record_level_color(level: &str) -> egui::Color32 {
        match level {
            "Purple" => egui::Color32::from_rgb(165, 88, 255),
            "Critical" => egui::Color32::from_rgb(220, 64, 52),
            "Warning" => egui::Color32::from_rgb(255, 180, 0),
            "Info" => egui::Color32::from_rgb(82, 170, 255),
            _ => egui::Color32::WHITE,
        }
    }

    pub(crate) fn draw_alarm_records_window(&mut self, ctx: &egui::Context) {
        if !self.alarm_records.open {
            return;
        }

        let mut open = self.alarm_records.open;
        egui::Window::new("报警记录")
            .open(&mut open)
            .default_size(egui::vec2(1040.0, 620.0))
            .min_width(820.0)
            .min_height(480.0)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("开始");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.alarm_records.start_ts_input)
                            .desired_width(172.0),
                    );
                    ui.label("结束");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.alarm_records.end_ts_input)
                            .desired_width(172.0),
                    );
                    if ui
                        .add_enabled(!self.alarm_records.loading, egui::Button::new("加载"))
                        .clicked()
                    {
                        self.request_alarm_records_load();
                    }
                    if ui.button("最近5分钟").clicked() {
                        let end_ms = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0);
                        self.alarm_records.start_ts_input = format_datetime_input(
                            end_ms.saturating_sub(ALARM_RECORD_DEFAULT_WINDOW_MS),
                        );
                        self.alarm_records.end_ts_input = format_datetime_input(end_ms);
                        self.alarm_records.page_index = 0;
                        self.alarm_records.has_next = false;
                        self.alarm_records.data = None;
                    }
                });

                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label("分组");
                    ui.toggle_value(&mut self.alarm_records.show_can_x, "CAN X");
                    ui.toggle_value(&mut self.alarm_records.show_can_y, "CAN Y");
                    ui.toggle_value(&mut self.alarm_records.show_can_z, "CAN Z");
                    ui.toggle_value(&mut self.alarm_records.show_can_timeout, "CAN Timeout");
                    ui.separator();
                    ui.toggle_value(&mut self.alarm_records.show_sent_t1, "SENT T1");
                    ui.toggle_value(&mut self.alarm_records.show_sent_t2, "SENT T2");
                    ui.toggle_value(&mut self.alarm_records.show_sent_s, "SENT S");
                });

                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(
                            !self.alarm_records.loading && self.alarm_records.page_index > 0,
                            egui::Button::new("上一页"),
                        )
                        .clicked()
                    {
                        self.request_alarm_records_page(self.alarm_records.page_index - 1);
                    }
                    ui.label(format!("第 {} 页", self.alarm_records.page_index + 1));
                    if ui
                        .add_enabled(
                            !self.alarm_records.loading && self.alarm_records.has_next,
                            egui::Button::new("下一页"),
                        )
                        .clicked()
                    {
                        self.request_alarm_records_page(self.alarm_records.page_index + 1);
                    }
                    ui.separator();
                    if ui.button("时间分布").clicked() {
                        self.open_can_replay_from_alarm_records();
                    }
                });

                ui.label(&self.alarm_records.status);
                ui.small(format!(
                    "按时间顺序显示，每页 {} 条，读取 alarm_events",
                    ALARM_RECORD_PAGE_SIZE
                ));
                ui.add_space(6.0);

                if let Some(data) = self.alarm_records.data.clone() {
                    ui.label(format!("本页 {} 条记录", data.records.len()));
                    ui.separator();
                    if data.records.is_empty() {
                        ui.vertical_centered(|ui| {
                            ui.add_space(80.0);
                            ui.heading("没有报警记录");
                        });
                    } else {
                        egui::ScrollArea::vertical()
                            .id_salt("alarm_records_db_scroll")
                            .show(ui, |ui| {
                                for row in &data.records {
                                    let color = Self::alarm_record_level_color(&row.level);
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label(format_datetime_input(row.ts_ms));
                                        ui.colored_label(color, &row.level);
                                        ui.label(if row.cleared {
                                            "已清除"
                                        } else {
                                            "已触发"
                                        });
                                        ui.label(Self::alarm_record_groups(row));
                                        ui.label(&row.device_id);
                                    });
                                    ui.label(
                                        egui::RichText::new(row.alarm_id.as_str())
                                            .monospace()
                                            .color(color),
                                    );
                                    ui.colored_label(color, &row.message);
                                    ui.separator();
                                }
                            });
                    }
                } else {
                    ui.group(|ui| {
                        ui.set_min_height(260.0);
                        ui.vertical_centered(|ui| {
                            ui.add_space(92.0);
                            ui.heading("报警记录");
                            ui.label("选择时间范围后点击加载");
                        });
                    });
                }
            });
        self.alarm_records.open = open;
    }
}
