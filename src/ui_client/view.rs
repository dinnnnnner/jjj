use crate::*;
use demo2::domain::AlarmLevel;

impl UiClientApp {
    pub(crate) fn sensor_label(&self, sensor_id: usize) -> String {
        let signal_id = format!("sensor_{sensor_id}_raw");
        if let Some(spec) = self.signal_processor.spec(&signal_id) {
            format!("{} [{}]", spec.name, spec.unit)
        } else {
            format!("Sensor {}", sensor_id)
        }
    }

    pub(crate) fn signal_value_text(&self, signal_id: &str, value: f64) -> String {
        if let Some(spec) = self.signal_processor.spec(signal_id) {
            format!("{:.*} {}", spec.decimals, value, spec.unit)
        } else {
            format!("{value:.3}")
        }
    }

    pub(crate) fn latest_demo_derived_angle_text(
        &self,
        binding: SignalBinding,
        device_id: &str,
    ) -> Option<String> {
        if device_id.is_empty() {
            return None;
        }
        let signal_id = binding.demo_derived_angle_id()?;
        let series = self
            .derived_signals
            .get(&(device_id.to_string(), signal_id.to_string()))?;
        let value = series.latest?;
        Some(self.signal_value_text(signal_id, value))
    }

    pub(crate) fn alarm_level_color(level: &AlarmLevel) -> egui::Color32 {
        match level {
            AlarmLevel::Info => egui::Color32::from_rgb(70, 140, 255),
            AlarmLevel::Warning => egui::Color32::from_rgb(255, 180, 0),
            AlarmLevel::Critical => egui::Color32::from_rgb(235, 64, 52),
            AlarmLevel::Purple => egui::Color32::from_rgb(165, 88, 255),
        }
    }

    pub(crate) fn alarm_level_rank(level: &AlarmLevel) -> u8 {
        match level {
            AlarmLevel::Info => 1,
            AlarmLevel::Warning => 2,
            AlarmLevel::Critical => 3,
            AlarmLevel::Purple => 4,
        }
    }

    pub(crate) fn active_alarm_summary_color(&self) -> egui::Color32 {
        match self
            .active_alarms
            .values()
            .map(|item| Self::alarm_level_rank(&item.event.level))
            .max()
        {
            Some(4) => Self::alarm_level_color(&AlarmLevel::Purple),
            Some(3) => Self::alarm_level_color(&AlarmLevel::Critical),
            Some(2) => Self::alarm_level_color(&AlarmLevel::Warning),
            Some(1) => Self::alarm_level_color(&AlarmLevel::Info),
            _ => egui::Color32::LIGHT_GREEN,
        }
    }

    pub(crate) fn alarm_level_text(level: &AlarmLevel) -> &'static str {
        match level {
            AlarmLevel::Info => "INFO",
            AlarmLevel::Warning => "WARNING",
            AlarmLevel::Critical => "CRITICAL",
            AlarmLevel::Purple => "PURPLE",
        }
    }

    pub(crate) fn demo_alarm_indicator(&self) -> Option<(&AlarmViewItem, bool)> {
        self.active_alarms
            .values()
            .find(|item| item.event.alarm_id == Self::DEMO_ALARM_ID)
            .map(|item| (item, true))
            .or_else(|| {
                self.alarm_history
                    .iter()
                    .find(|item| item.event.alarm_id == Self::DEMO_ALARM_ID)
                    .map(|item| (item, false))
            })
    }

    pub(crate) fn draw_demo_alarm_indicator(&self, ctx: &egui::Context) {
        if self.selected_view != TestSignalView::Demo {
            return;
        }

        egui::Window::new("demo_alarm_indicator")
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .fixed_pos(egui::pos2(424.0, 90.0))
            .fixed_size(egui::vec2(210.0, 96.0))
            .show(ctx, |ui| {
                let (label, color, detail) = match self.demo_alarm_indicator() {
                    Some((_, true)) => (
                        "ALARM",
                        egui::Color32::from_rgb(220, 64, 52),
                        format!("bit={}", self.demo_alarm_bit_state.unwrap_or(true)),
                    ),
                    Some((_, false)) => (
                        "CLEAR",
                        egui::Color32::from_rgb(64, 170, 92),
                        format!("bit={}", self.demo_alarm_bit_state.unwrap_or(false)),
                    ),
                    None => (
                        "NORMAL",
                        egui::Color32::from_rgb(64, 170, 92),
                        format!("bit={}", self.demo_alarm_bit_state.unwrap_or(false)),
                    ),
                };

                ui.vertical_centered(|ui| {
                    ui.heading("DEMO Alarm");
                    ui.add_space(6.0);
                    ui.colored_label(color, egui::RichText::new(label).size(28.0).strong());
                    ui.label(detail);
                });
            });
    }

    /*    fn draw_can_alarm_indicator(&self, ctx: &egui::Context) {
            if self.selected_view != TestSignalView::CanFrame {
                return;
            }

            egui::Window::new("告警指示器")
                .title_bar(true)
                .resizable(false)
                .collapsible(false)
                .default_pos(egui::pos2(648.0, 40.0))
                .fixed_size(egui::vec2(260.0, 110.0))
                .show(ctx, |ui| {
                    let (label, color, detail, message) = match self.can_alarm_indicator() {
                        Some((item, true)) => (
                            "告警",
                            Self::alarm_level_color(&item.event.level),
                            "当前激活".to_string(),
                            format!("{} | {}", item.event.alarm_id, item.event.device_id),
                        ),
                        Some((item, false)) => (
                            "已清除",
                            egui::Color32::from_rgb(64, 170, 92),
                            "最近一次".to_string(),
                            format!("{} | {}", item.event.alarm_id, item.event.device_id),
                        ),
                        None => (
                            "正常",
                            egui::Color32::from_rgb(64, 170, 92),
                            "没有can告警".to_string(),
                            "等待告警事件...".to_string(),
                        ),
                    };

                    ui.vertical_centered(|ui| {
                        ui.heading("CAN 告警");
                        ui.add_space(6.0);
                        ui.colored_label(color, egui::RichText::new(label).size(28.0).strong());
                        ui.label(detail);
                        ui.small(message);
                    });
                });
        }

    */
    pub(crate) fn can_alarm_indicator(&self) -> Option<(&AlarmViewItem, bool)> {
        self.active_alarms
            .values()
            .find(|item| item.event.device_id.starts_with("can://"))
            .map(|item| (item, true))
            .or_else(|| {
                self.alarm_history
                    .iter()
                    .find(|item| item.event.device_id.starts_with("can://"))
                    .map(|item| (item, false))
            })
    }

    pub(crate) fn draw_can_alarm_indicator(&self, ctx: &egui::Context) {
        if self.selected_view != TestSignalView::CanFrame {
            return;
        }

        egui::Window::new("CAN Alarm")
            .title_bar(true)
            .resizable(false)
            .collapsible(false)
            .default_pos(egui::pos2(648.0, 40.0))
            .fixed_size(egui::vec2(260.0, 110.0))
            .show(ctx, |ui| {
                let (label, color, detail, message) = match self.can_alarm_indicator() {
                    Some((item, true)) => (
                        "告警",
                        Self::alarm_level_color(&item.event.level),
                        "当前激活".to_string(),
                        format!("{} | {}", item.event.alarm_id, item.event.device_id),
                    ),
                    Some((item, false)) => (
                        "已清除",
                        egui::Color32::from_rgb(64, 170, 92),
                        "最近一次".to_string(),
                        format!("{} | {}", item.event.alarm_id, item.event.device_id),
                    ),
                    None => (
                        "正常",
                        egui::Color32::from_rgb(64, 170, 92),
                        "没有 CAN 告警".to_string(),
                        "等待告警事件...".to_string(),
                    ),
                };

                ui.vertical_centered(|ui| {
                    ui.heading("CAN 告警");
                    ui.add_space(6.0);
                    ui.colored_label(color, egui::RichText::new(label).size(28.0).strong());
                    ui.label(detail);
                    ui.small(message);
                });
            });
    }

    pub(crate) fn sent_alarm_indicator(&self) -> Option<(&AlarmViewItem, bool)> {
        self.active_alarms
            .values()
            .find(|item| item.event.alarm_id.starts_with("sent_error_"))
            .map(|item| (item, true))
    }

    pub(crate) fn draw_sent_alarm_indicator(&self, ctx: &egui::Context) {
        if self.selected_view != TestSignalView::Sent {
            return;
        }

        egui::Window::new("SENT Alarm")
            .title_bar(true)
            .resizable(false)
            .collapsible(false)
            .default_pos(egui::pos2(648.0, 40.0))
            .fixed_size(egui::vec2(280.0, 110.0))
            .show(ctx, |ui| {
                let (label, color, detail, message) = match self.sent_alarm_indicator() {
                    Some((item, true)) => (
                        "ALARM",
                        Self::alarm_level_color(&item.event.level),
                        "active".to_string(),
                        format!("{} | {}", item.event.alarm_id, item.event.message),
                    ),
                    Some((item, false)) => (
                        "LAST",
                        egui::Color32::from_rgb(255, 180, 0),
                        "latest SENT alarm".to_string(),
                        format!("{} | {}", item.event.alarm_id, item.event.message),
                    ),
                    None => (
                        "OK",
                        egui::Color32::from_rgb(64, 170, 92),
                        "no SENT alarm".to_string(),
                        "waiting for SENT error frame...".to_string(),
                    ),
                };

                ui.vertical_centered(|ui| {
                    ui.heading("SENT Alarm");
                    ui.add_space(6.0);
                    ui.colored_label(color, egui::RichText::new(label).size(28.0).strong());
                    ui.label(detail);
                    ui.small(message);
                });
            });
    }

    pub(crate) fn sent_jump_color(level: SentJumpLevel) -> egui::Color32 {
        match level {
            SentJumpLevel::Normal => egui::Color32::from_rgb(64, 170, 92),
            SentJumpLevel::Warning => egui::Color32::from_rgb(255, 180, 0),
            SentJumpLevel::Critical => egui::Color32::from_rgb(220, 64, 52),
            SentJumpLevel::Purple => egui::Color32::from_rgb(165, 88, 255),
        }
    }

    pub(crate) fn sent_jump_level_text(level: SentJumpLevel) -> &'static str {
        match level {
            SentJumpLevel::Normal => "OK",
            SentJumpLevel::Warning => "WARN",
            SentJumpLevel::Critical => "RED",
            SentJumpLevel::Purple => "PURPLE",
        }
    }

    pub(crate) fn sent_jump_alarm_id(sensor_id: usize) -> &'static str {
        if sensor_id == 1 {
            "sent_torque_jump_t1"
        } else {
            "sent_torque_jump_t2"
        }
    }

    pub(crate) fn sent_angle_jump_alarm_id(sensor_id: usize) -> &'static str {
        match sensor_id {
            0 => "sent_angle_jump_t1",
            2 => "sent_angle_jump_t2",
            4 => "sent_angle_jump_s",
            _ => "sent_angle_jump_unknown",
        }
    }

    pub(crate) fn sent_jump_level_for_alarm(&self, alarm_id: &str) -> SentJumpLevel {
        self.active_alarms
            .values()
            .filter(|item| item.event.alarm_id == alarm_id)
            .map(|item| match item.event.level {
                AlarmLevel::Purple => SentJumpLevel::Purple,
                AlarmLevel::Critical => SentJumpLevel::Critical,
                AlarmLevel::Warning | AlarmLevel::Info => SentJumpLevel::Warning,
            })
            .max_by_key(|level| match level {
                SentJumpLevel::Purple => 3,
                SentJumpLevel::Critical => 2,
                SentJumpLevel::Warning => 1,
                SentJumpLevel::Normal => 0,
            })
            .unwrap_or(SentJumpLevel::Normal)
    }

    pub(crate) fn alarm_raise_count(&self, alarm_id: &str) -> usize {
        self.alarm_history
            .iter()
            .filter(|item| item.event.alarm_id == alarm_id && !item.event.cleared)
            .count()
    }

    pub(crate) fn sent_angle_alarm_active(&self, alarm_id: &str) -> bool {
        self.active_alarms
            .values()
            .any(|item| item.event.alarm_id == alarm_id)
    }

    pub(crate) fn draw_sent_jump_indicator(&self, ctx: &egui::Context) {
        if self.selected_view != TestSignalView::Sent {
            return;
        }

        egui::Window::new("SENT Torque Jump")
            .title_bar(true)
            .resizable(false)
            .collapsible(false)
            .default_pos(egui::pos2(648.0, 160.0))
            .fixed_size(egui::vec2(280.0, 145.0))
            .show(ctx, |ui| {
                ui.heading("SENT torque jump");
                ui.add_space(6.0);
                for sensor_id in [1usize, 3usize] {
                    let alarm_id = Self::sent_jump_alarm_id(sensor_id);
                    let level = self.sent_jump_level_for_alarm(alarm_id);
                    let color = Self::sent_jump_color(level);
                    ui.horizontal(|ui| {
                        ui.colored_label(color, egui::RichText::new("●").size(24.0));
                        ui.label(Self::sent_torque_label(sensor_id));
                        ui.label(Self::sent_jump_level_text(level));
                    });
                    ui.small(format!("alert_count={}", self.alarm_raise_count(alarm_id)));
                }
            });
    }

    pub(crate) fn draw_sent_angle_jump_indicator(&self, ctx: &egui::Context) {
        if self.selected_view != TestSignalView::Sent {
            return;
        }

        egui::Window::new("SENT Angle Jump")
            .title_bar(true)
            .resizable(false)
            .collapsible(false)
            .default_pos(egui::pos2(648.0, 320.0))
            .fixed_size(egui::vec2(280.0, 130.0))
            .show(ctx, |ui| {
                ui.heading("SENT angle jump");
                ui.add_space(6.0);
                for sensor_id in [0usize, 2usize, 4usize] {
                    let alarm_id = Self::sent_angle_jump_alarm_id(sensor_id);
                    let active = self.sent_angle_alarm_active(alarm_id);
                    let color = if active {
                        egui::Color32::from_rgb(220, 64, 52)
                    } else {
                        egui::Color32::from_rgb(64, 170, 92)
                    };
                    ui.horizontal(|ui| {
                        ui.colored_label(color, egui::RichText::new("●").size(24.0));
                        ui.label(Self::sent_angle_label(sensor_id));
                        ui.label(if active { "RED" } else { "OK" });
                    });
                    ui.small(format!("alert_count={}", self.alarm_raise_count(alarm_id)));
                }
            });
    }

    pub(crate) fn draw_sent_jump_threshold_panel(&mut self, ctx: &egui::Context) {
        if self.selected_view != TestSignalView::Sent {
            return;
        }

        egui::Window::new("SENT Thresholds")
            .title_bar(true)
            .resizable(false)
            .collapsible(false)
            .default_pos(egui::pos2(940.0, 160.0))
            .fixed_size(egui::vec2(300.0, 178.0))
            .show(ctx, |ui| {
                ui.heading("SENT torque jump thresholds");
                ui.add_space(6.0);
                egui::Grid::new("sent_jump_threshold_grid")
                    .num_columns(2)
                    .spacing(egui::vec2(8.0, 8.0))
                    .show(ui, |ui| {
                        ui.label("yellow >");
                        ui.add(
                            egui::TextEdit::singleline(
                                &mut self.sent_jump_thresholds.warn_input,
                            )
                            .desired_width(96.0),
                        );
                        ui.end_row();

                        ui.label("red >=");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.sent_jump_thresholds.red_input)
                                .desired_width(96.0),
                        );
                        ui.end_row();

                        ui.label("purple >=");
                        ui.add(
                            egui::TextEdit::singleline(
                                &mut self.sent_jump_thresholds.purple_input,
                            )
                            .desired_width(96.0),
                        );
                        ui.end_row();
                    });

                ui.add_space(8.0);
                if ui.button("Apply").clicked() {
                    self.status = match self.apply_sent_jump_thresholds() {
                        Ok(()) => "SENT torque jump thresholds applied".to_string(),
                        Err(err) => format!("SENT torque jump threshold error: {err}"),
                    };
                }
                let (warn, red, purple) = self.sent_jump_thresholds();
                ui.small(format!(
                    "current: green <= {warn:.3}, yellow > {warn:.3}, red >= {red:.3}, purple >= {purple:.3}"
                ));
            });
    }

    pub(crate) fn draw_sent_angle_jump_threshold_panel(&mut self, ctx: &egui::Context) {
        if self.selected_view != TestSignalView::Sent {
            return;
        }

        egui::Window::new("SENT Angle Threshold")
            .title_bar(true)
            .resizable(false)
            .collapsible(false)
            .default_pos(egui::pos2(940.0, 352.0))
            .fixed_size(egui::vec2(320.0, 176.0))
            .show(ctx, |ui| {
                ui.heading("SENT angle jump threshold");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("T1 red >=");
                    ui.add(
                        egui::TextEdit::singleline(
                            &mut self.sent_angle_jump_thresholds.t1_red_input,
                        )
                        .desired_width(96.0),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label("T2 red >=");
                    ui.add(
                        egui::TextEdit::singleline(
                            &mut self.sent_angle_jump_thresholds.t2_red_input,
                        )
                        .desired_width(96.0),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label("S red >=");
                    ui.add(
                        egui::TextEdit::singleline(
                            &mut self.sent_angle_jump_thresholds.s_red_input,
                        )
                        .desired_width(96.0),
                    );
                });

                ui.add_space(8.0);
                if ui.button("Apply").clicked() {
                    self.status = match self.apply_sent_angle_jump_threshold() {
                        Ok(()) => "SENT angle jump threshold applied".to_string(),
                        Err(err) => format!("SENT angle jump threshold error: {err}"),
                    };
                }
                let t1_red = self.sent_angle_jump_threshold(0);
                let t2_red = self.sent_angle_jump_threshold(2);
                let s_red = self.sent_angle_jump_threshold(4);
                ui.small(format!(
                    "current: T1 >= {t1_red:.3}, T2 >= {t2_red:.3}, S >= {s_red:.3}"
                ));
            });
    }

    pub(crate) fn draw_can_threshold_panel(&mut self, ctx: &egui::Context) {
        if self.selected_view != TestSignalView::CanFrame {
            return;
        }

        egui::Window::new("CAN 参考线")
            .title_bar(true)
            .resizable(false)
            .collapsible(false)
            .default_pos(egui::pos2(924.0, 0.0))
            .fixed_size(egui::vec2(400.0, 190.0))
            .show(ctx, |ui| {
                ui.heading("CAN 图表参考线");

                ui.add_space(6.0);

                egui::Grid::new("can_alarm_threshold_grid")
                    .num_columns(3)
                    .spacing(egui::vec2(8.0, 8.0))
                    .show(ui, |ui| {
                        ui.label("轴");
                        ui.label("上限");
                        ui.label("下限");
                        ui.end_row();

                        ui.label("X");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.can_alarm_thresholds.x_high_input)
                                .desired_width(88.0),
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut self.can_alarm_thresholds.x_low_input)
                                .desired_width(40.0),
                        );
                        ui.end_row();

                        ui.label("Y");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.can_alarm_thresholds.y_high_input)
                                .desired_width(88.0),
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut self.can_alarm_thresholds.y_low_input)
                                .desired_width(40.0),
                        );
                        ui.end_row();

                        ui.label("Z");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.can_alarm_thresholds.z_high_input)
                                .desired_width(88.0),
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut self.can_alarm_thresholds.z_low_input)
                                .desired_width(40.0),
                        );
                        ui.end_row();
                    });

                ui.add_space(8.0);
                if ui.button("确认阈值").clicked() {
                    self.status = match self.apply_can_alarm_thresholds() {
                        Ok(()) => "CAN 图表参考线已确认生效".to_string(),
                        Err(err) => format!("CAN 图表参考线确认失败: {err}"),
                    };
                }
            });
    }

    pub(crate) fn clear_alarm_panel_stats(&mut self) {
        self.active_alarms.clear();
        self.alarm_history.clear();
        self.total_alarm_count = 0;
        self.status = "报警面板和报警统计已清除".to_string();
    }

    pub(crate) fn draw_alarm_panel(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("alarm_panel")
            .resizable(true)
            .default_height(300.0)
            .min_height(520.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(format!("报警次数: {}", self.total_alarm_count));
                    ui.separator();
                    if ui.button("清除报警").clicked() {
                        self.clear_alarm_panel_stats();
                    }
                    ui.separator();
                    ui.heading("报警");
                    ui.separator();
                    ui.colored_label(
                        self.active_alarm_summary_color(),
                        format!("当前活跃: {}", self.active_alarms.len()),
                    );
                    ui.label(format!("最近记录: {}", self.alarm_history.len()));
                });

                ui.add_space(6.0);
                ui.columns(2, |columns| {
                    columns[0].group(|ui| {
                        ui.label("当前报警");
                        ui.separator();
                        if self.active_alarms.is_empty() {
                            ui.label("暂无活跃报警");
                        } else {
                            egui::ScrollArea::vertical()
                                .id_salt("active_alarm_scroll")
                                .show(ui, |ui| {
                                    let mut items: Vec<_> = self.active_alarms.values().collect();
                                    items.sort_by_key(|item| std::cmp::Reverse(item.received_at));
                                    for item in items {
                                        let level_color =
                                            Self::alarm_level_color(&item.event.level);
                                        ui.horizontal_wrapped(|ui| {
                                            ui.colored_label(
                                                level_color,
                                                Self::alarm_level_text(&item.event.level),
                                            );
                                            ui.label(format_alarm_datetime(item.event.raised_at));
                                            ui.label(&item.event.device_id);
                                            ui.label(
                                                egui::RichText::new(item.event.alarm_id.as_str())
                                                    .monospace()
                                                    .color(level_color),
                                            );
                                        });
                                        ui.colored_label(level_color, &item.event.message);
                                        ui.separator();
                                    }
                                });
                        }
                    });

                    columns[1].group(|ui| {
                        ui.label("最近报警记录");
                        ui.separator();
                        if self.alarm_history.is_empty() {
                            ui.label("还没有收到报警事件");
                        } else {
                            egui::ScrollArea::vertical()
                                .id_salt("alarm_history_scroll")
                                .show(ui, |ui| {
                                    for item in &self.alarm_history {
                                        let level_color =
                                            Self::alarm_level_color(&item.event.level);
                                        ui.horizontal_wrapped(|ui| {
                                            ui.colored_label(
                                                level_color,
                                                Self::alarm_level_text(&item.event.level),
                                            );
                                            ui.label(format_alarm_datetime(item.event.raised_at));
                                            ui.label(if item.event.cleared {
                                                "已恢复"
                                            } else {
                                                "已触发"
                                            });
                                            ui.label(&item.event.device_id);
                                        });
                                        ui.label(
                                            egui::RichText::new(item.event.alarm_id.as_str())
                                                .monospace()
                                                .color(level_color),
                                        );
                                        ui.colored_label(level_color, &item.event.message);
                                        ui.separator();
                                    }
                                });
                        }
                    });
                });
            });
    }

    pub(crate) fn push_dynamic_window(&mut self, title: String, binding: Option<SignalBinding>) {
        let position = egui::pos2(
            120.0 + self.dynamic_windows.len() as f32 * 28.0,
            120.0 + self.dynamic_windows.len() as f32 * 24.0,
        );
        self.dynamic_windows.push(DynamicSignalWindow {
            title,
            binding,
            position,
            scale: 1.0,
            rect: None,
        });
    }

    pub(crate) fn add_dynamic_window(&mut self) {
        match self.selected_view {
            TestSignalView::Demo => {
                let index = self.demo_group_index;
                self.push_dynamic_window(
                    SignalBinding::DemoAxisX.title(index),
                    Some(SignalBinding::DemoAxisX),
                );
                self.push_dynamic_window(
                    SignalBinding::DemoAxisY.title(index),
                    Some(SignalBinding::DemoAxisY),
                );
                self.push_dynamic_window(
                    SignalBinding::DemoAxisZ.title(index),
                    Some(SignalBinding::DemoAxisZ),
                );
                self.demo_group_index = self.demo_group_index.saturating_add(1);
            }
            TestSignalView::Sent => {
                let index = self.sent1_group_index;
                self.push_dynamic_window(
                    SignalBinding::Sent1V1.title(index),
                    Some(SignalBinding::Sent1V1),
                );
                self.push_dynamic_window(
                    SignalBinding::Sent1P1.title(index),
                    Some(SignalBinding::Sent1P1),
                );
                self.push_dynamic_window(
                    SignalBinding::Sent2V2.title(index),
                    Some(SignalBinding::Sent2V2),
                );
                self.push_dynamic_window(
                    SignalBinding::Sent2P2.title(index),
                    Some(SignalBinding::Sent2P2),
                );
                self.push_dynamic_window(
                    SignalBinding::Sent3Angle.title(index),
                    Some(SignalBinding::Sent3Angle),
                );
                self.sent1_group_index = self.sent1_group_index.saturating_add(1);
            }
            TestSignalView::CanFrame => {
                let index = self.can_group_index;
                self.push_dynamic_window(
                    SignalBinding::CanAxisX.title(index),
                    Some(SignalBinding::CanAxisX),
                );
                self.push_dynamic_window(
                    SignalBinding::CanAxisY.title(index),
                    Some(SignalBinding::CanAxisY),
                );
                self.push_dynamic_window(
                    SignalBinding::CanAxisZ.title(index),
                    Some(SignalBinding::CanAxisZ),
                );
                self.can_group_index = self.can_group_index.saturating_add(1);
            }
            TestSignalView::TcpFrame => {
                let index = self.tcp_group_index;
                let start_sensor = self
                    .dynamic_windows
                    .iter()
                    .filter_map(|window| match window.binding {
                        Some(SignalBinding::TcpSensor(sensor_id)) => Some(sensor_id),
                        _ => None,
                    })
                    .max()
                    .unwrap_or(0);
                for sensor_id in start_sensor..start_sensor.saturating_add(4) {
                    let binding = SignalBinding::TcpSensor(sensor_id);
                    self.push_dynamic_window(binding.title(index), Some(binding));
                }
                self.tcp_group_index = self.tcp_group_index.saturating_add(1);
            }
        }
    }

    pub(crate) fn apply_ctrl_wheel_zoom(&mut self, ctx: &egui::Context) {
        let (ctrl, scroll_y, pointer_pos) = ctx.input(|i| {
            (
                i.modifiers.ctrl,
                i.raw_scroll_delta.y,
                i.pointer.hover_pos(),
            )
        });

        if !ctrl || scroll_y.abs() < f32::EPSILON {
            return;
        }
        let Some(pointer) = pointer_pos else {
            return;
        };

        for window in &mut self.dynamic_windows {
            if let Some(rect) = window.rect {
                if rect.contains(pointer) {
                    let factor = (1.0 + scroll_y * 0.0015).clamp(0.85, 1.2);
                    window.scale = (window.scale * factor).clamp(SCALE_MIN, SCALE_MAX);
                    break;
                }
            }
        }
    }

    fn draw_sensor_chart(
        ui: &mut egui::Ui,
        points: &VecDeque<[f64; 2]>,
        height: f32,
        label: &str,
        thresholds: Option<(Option<f64>, Option<f64>)>,
    ) {
        let desired_size = egui::vec2(ui.available_width(), height.max(40.0));
        let (rect, _) = ui.allocate_exact_size(desired_size, egui::Sense::hover());
        let painter = ui.painter_at(rect);
        let max_chart_points = (rect.width() * CHART_POINTS_PER_PIXEL).max(4.0).round() as usize;
        let chart_points = super::series::downsample_for_chart(points, max_chart_points);

        painter.rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(1.0, egui::Color32::DARK_GRAY),
            egui::StrokeKind::Outside,
        );

        let (high_threshold, low_threshold) = thresholds.unwrap_or((None, None));
        if points.len() < 2 && high_threshold.is_none() && low_threshold.is_none() {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(12.0),
                egui::Color32::GRAY,
            );
            return;
        }

        let min_x = points.front().map(|p| p[0]).unwrap_or(0.0);
        let max_x = points.back().map(|p| p[0]).unwrap_or(min_x + 1.0);
        let mut min_y = f64::INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        for p in points {
            min_y = min_y.min(p[1]);
            max_y = max_y.max(p[1]);
        }
        if !min_y.is_finite() || !max_y.is_finite() {
            min_y = -1.0;
            max_y = 1.0;
        }
        if (max_y - min_y).abs() < f64::EPSILON {
            max_y += 1.0;
            min_y -= 1.0;
        }
        let pad = (max_y - min_y) * Y_RANGE_PADDING_RATIO;
        max_y += pad;
        min_y -= pad;

        let to_screen = |x: f64, y: f64| -> egui::Pos2 {
            let tx = ((x - min_x) / (max_x - min_x + f64::EPSILON)) as f32;
            let ty = ((y - min_y) / (max_y - min_y + f64::EPSILON)) as f32;
            egui::pos2(
                rect.left() + tx * rect.width(),
                rect.bottom() - ty * rect.height(),
            )
        };

        let draw_threshold_line = |value: f64, label: &str| {
            let y = to_screen(min_x, value)
                .y
                .clamp(rect.top() + 2.0, rect.bottom() - 2.0);
            let color = egui::Color32::from_rgb(220, 64, 52);
            painter.line_segment(
                [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                egui::Stroke::new(1.1, color),
            );
            painter.text(
                egui::pos2(rect.right() - 4.0, y - 2.0),
                egui::Align2::RIGHT_BOTTOM,
                label,
                egui::FontId::proportional(10.0),
                color,
            );
        };

        if let Some(value) = high_threshold {
            draw_threshold_line(value, &format!("H {:.2}", value));
        }
        if let Some(value) = low_threshold {
            draw_threshold_line(value, &format!("L {:.2}", value));
        }

        let display_max_y = match high_threshold {
            Some(value) if value > max_y => value,
            _ => max_y,
        };
        let display_min_y = match low_threshold {
            Some(value) if value < min_y => value,
            _ => min_y,
        };

        let mut segment: Vec<egui::Pos2> = Vec::with_capacity(chart_points.len());
        let mut prev_t: Option<f64> = None;
        for p in &chart_points {
            if let Some(pt) = prev_t {
                if p[0] - pt > LINE_BREAK_GAP_SECS {
                    if segment.len() >= 2 {
                        painter.add(egui::Shape::line(
                            std::mem::take(&mut segment),
                            egui::Stroke::new(1.6, egui::Color32::LIGHT_GREEN),
                        ));
                    } else {
                        segment.clear();
                    }
                }
            }
            segment.push(to_screen(p[0], p[1]));
            prev_t = Some(p[0]);
        }
        if segment.len() >= 2 {
            painter.add(egui::Shape::line(
                segment,
                egui::Stroke::new(1.6, egui::Color32::LIGHT_GREEN),
            ));
        }

        // Show dynamic y-bounds for this rolling chart window.
        painter.text(
            rect.left_top() + egui::vec2(6.0, 4.0),
            egui::Align2::LEFT_TOP,
            format!("上界 {:.3}", display_max_y),
            egui::FontId::proportional(11.0),
            egui::Color32::LIGHT_BLUE,
        );
        painter.text(
            rect.left_bottom() + egui::vec2(6.0, -4.0),
            egui::Align2::LEFT_BOTTOM,
            format!("下界 {:.3}", display_min_y),
            egui::FontId::proportional(11.0),
            egui::Color32::LIGHT_BLUE,
        );

        // Mark latest point and keep its value label moving with the point.
        if let Some(last) = points.back() {
            let p = to_screen(last[0], last[1]);
            painter.circle_filled(p, 3.5, egui::Color32::YELLOW);

            let label_pos = egui::pos2(
                (p.x + 8.0).clamp(rect.left() + 6.0, rect.right() - 70.0),
                (p.y - 8.0).clamp(rect.top() + 16.0, rect.bottom() - 6.0),
            );
            painter.line_segment([p, label_pos], egui::Stroke::new(1.0, egui::Color32::GOLD));
            painter.text(
                label_pos,
                egui::Align2::LEFT_BOTTOM,
                format!("{:.3}", last[1]),
                egui::FontId::proportional(11.0),
                egui::Color32::YELLOW,
            );
        }
    }

    fn send_can_self_test(&mut self) {
        self.can_self_test_counter = self.can_self_test_counter.wrapping_add(1);
        let data = SELF_TEST_CAN_DATA;
        let frame = CanTxFrame::new(0, SELF_TEST_CAN_ID, SELF_TEST_CAN_DLC, data);
        match enqueue_can_tx(frame) {
            Ok(()) => {
                let message = format!(
                    "CAN self-test queued: id=0x{:X} dlc={} data={:02X?}，等待 collector 回报自检结果",
                    SELF_TEST_CAN_ID, SELF_TEST_CAN_DLC, data
                );
                self.last_can_self_test_result = message.clone();
                self.status = message;
            }
            Err(err) => {
                let message = format!("CAN self-test failed: {err}");
                self.last_can_self_test_result = message.clone();
                self.status = message;
            }
        }
    }
}

impl eframe::App for UiClientApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let feed_backlog = self.drain_messages();
        self.apply_ctrl_wheel_zoom(ctx);

        egui::TopBottomPanel::top("self_test_result_top")
            .resizable(false)
            .default_height(24.0)
            .show(ctx, |ui| {
                ui.label(format!("最近自检结果: {}", self.last_can_self_test_result));
            });

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.heading("UI Client");
            ui.label(format!("collector feed: {}", self.feed_addr));
            ui.label("serial data path: serial -> collector_service -> ui_client");
            ui.horizontal(|ui| {
                if ui.button("重排窗口并重置缩放").clicked() {
                    self.reset_layout();
                }
                if ui
                    .add_enabled(
                        !self.dynamic_windows.is_empty(),
                        egui::Button::new("删除全部显示框"),
                    )
                    .clicked()
                {
                    self.dynamic_windows.clear();
                }
                if ui.button("CAN自检").clicked() {
                    self.send_can_self_test();
                }
                if ui.button("CAN回放").clicked() {
                    self.open_can_replay();
                }
                if ui.button("报警记录").clicked() {
                    self.open_alarm_records();
                }
            });
            ui.horizontal(|ui| {
                ui.label("Test signal");
                ui.selectable_value(&mut self.selected_view, TestSignalView::Demo, "DEMO");
                ui.selectable_value(&mut self.selected_view, TestSignalView::Sent, "SENT");
                ui.selectable_value(&mut self.selected_view, TestSignalView::CanFrame, "CAN");
                ui.selectable_value(&mut self.selected_view, TestSignalView::TcpFrame, "TCP");
                if ui.button("Add").clicked() {
                    self.add_dynamic_window();
                }
            });
            ui.label(format!("状态: {}", self.status));
            ui.label(format!("总样本数: {}", self.total_samples));
            ui.label(format!(
                "丢弃的消息(ui队列满): {}",
                self.feed_stats.dropped_messages.load(Ordering::Relaxed)
            ));
            ui.label(format!(
                "解码失败数: {}",
                self.feed_stats.decode_errors.load(Ordering::Relaxed)
            ));
            ui.label(format!("最新请求ID: {}", self.last_req));
        });

        self.draw_alarm_panel(ctx);
        self.draw_demo_alarm_indicator(ctx);
        self.draw_sent_alarm_indicator(ctx);
        self.draw_sent_jump_indicator(ctx);
        self.draw_sent_jump_threshold_panel(ctx);
        self.draw_sent_angle_jump_indicator(ctx);
        self.draw_sent_angle_jump_threshold_panel(ctx);
        self.draw_can_alarm_indicator(ctx);
        self.draw_can_threshold_panel(ctx);
        self.draw_can_replay_window(ctx);
        self.draw_alarm_records_window(ctx);

        let mut remove_idx = Vec::new();
        for idx in 0..self.dynamic_windows.len() {
            let title = self.dynamic_windows[idx].title.clone();
            let binding = self.dynamic_windows[idx].binding;
            let start_pos = self.dynamic_windows[idx].position;
            let scale = self.dynamic_windows[idx].scale;
            let id = egui::Id::new(format!("dynamic_signal_window_{idx}"));
            let mut current_pos = start_pos;
            let mut open = true;
            let win_w = 290.0 * scale;
            let win_h = 220.0 * scale;
            let chart_h = 120.0 * scale;
            let response = egui::Window::new(title)
                .id(id)
                .open(&mut open)
                .current_pos(start_pos)
                .movable(true)
                .resizable(false)
                .collapsible(false)
                .fixed_size(egui::vec2(win_w, win_h))
                .show(ctx, |ui| {
                    if let Some(binding) = binding {
                        let sensor_id = binding.sensor_id();
                        if sensor_id >= SENSOR_COUNT {
                            ui.label(format!("sensor {sensor_id} 超出当前可用范围"));
                            return;
                        }
                        let raw_signal_id = format!("sensor_{sensor_id}_raw");
                        let series = if binding.uses_tcp_series() {
                            &self.tcp_sensors[sensor_id]
                        } else {
                            &self.sensors[sensor_id]
                        };
                        if let Some(v) = series.latest {
                            let text = if binding.is_can_axis() {
                                format!("{:.0}", v)
                            } else {
                                self.signal_value_text(&raw_signal_id, v)
                            };
                            ui.label(format!("{}: {}", binding.value_label(), text));
                        } else {
                            ui.label(format!("{}: N/A", binding.value_label()));
                        }
                        if let Some(text) =
                            self.latest_demo_derived_angle_text(binding, &series.device_id)
                        {
                            ui.label(format!("angle: {}", text));
                        }
                        if !series.device_id.is_empty() {
                            ui.label(format!("device: {}", series.device_id));
                        }
                        let chart_label = binding
                            .chart_label()
                            .map(str::to_string)
                            .unwrap_or_else(|| self.sensor_label(sensor_id));
                        let chart_thresholds = self.can_chart_thresholds(binding);
                        Self::draw_sensor_chart(
                            ui,
                            &series.points,
                            chart_h,
                            &format!("{chart_label} (last {:.0}s)", WINDOW_SECS),
                            chart_thresholds,
                        );
                    } else {
                        ui.label("No signal mapping for this mode yet.");
                    }
                });

            if let Some(inner) = response {
                current_pos = inner.response.rect.min;
                self.dynamic_windows[idx].rect = Some(inner.response.rect);
            } else {
                self.dynamic_windows[idx].rect = None;
            }
            self.dynamic_windows[idx].position = current_pos;
            if !open {
                remove_idx.push(idx);
            }
        }

        for idx in remove_idx.into_iter().rev() {
            self.dynamic_windows.remove(idx);
        }

        if feed_backlog {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }
}
