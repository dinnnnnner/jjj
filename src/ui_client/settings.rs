pub(crate) const DISPLAY_TZ_OFFSET_SECS: i64 = 8 * 3600;
pub(crate) const SENSOR_COUNT: usize = 10;
pub(crate) const WINDOW_SECS: f64 = 15.0;
pub(crate) const LINE_BREAK_GAP_SECS: f64 = 2.0;
pub(crate) const Y_RANGE_PADDING_RATIO: f64 = 0.08;
pub(crate) const SCALE_MIN: f32 = 0.6;
pub(crate) const SCALE_MAX: f32 = 2.4;
pub(crate) const MAX_POINTS_PER_SERIES: usize = 4096;
// Keep message bursts from monopolizing egui's update thread. At the normal
// 60 Hz repaint rate this still leaves ample headroom above a 2,500 fps feed.
pub(crate) const MAX_UI_MESSAGES_PER_UPDATE: usize = 1024;
pub(crate) const UI_MESSAGE_TIME_BUDGET_MS: u64 = 6;
// More vertices than this cannot add useful detail to the on-screen chart.
pub(crate) const CHART_POINTS_PER_PIXEL: f32 = 2.0;
pub(crate) const UI_QUEUE_CAPACITY: usize = 50_000;
pub(crate) const ALARM_RECORD_DEFAULT_WINDOW_MS: i64 = 5 * 60 * 1000;
pub(crate) const ALARM_RECORD_PAGE_SIZE: i64 = 50;
pub(crate) const ALARM_REPLAY_CONTEXT_MS: i64 = 30 * 1000;
pub(crate) const SELF_TEST_CAN_ID: u32 = 0x123;
pub(crate) const SELF_TEST_CAN_DLC: u8 = 8;
pub(crate) const SELF_TEST_CAN_DATA: [u8; 8] = [0xA5, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
pub(crate) const CAN_REPLAY_DEFAULT_WINDOW_MS: i64 = 5 * 60 * 1000;
pub(crate) const CAN_REPLAY_MIN_SPAN_SEC: f64 = 1.0;

pub(crate) const CAN_EXPORT_DIR: &str = "exports";
