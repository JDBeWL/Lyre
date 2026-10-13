//! 极简文件日志：把 `log` 记录写到 `<数据目录>/logs/app.log`。
//! release 下 `windows_subsystem = "windows"` 让 stderr 无处可去；文件打不开时退回 stderr。

use std::fs::{self, File, OpenOptions};
use std::io::Write;

use crate::persist::data_dir;

struct FileLogger {
    file: Option<File>,
}

impl log::Log for FileLogger {
    fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        let line = format!(
            "[{}] {:<5} {}\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            record.level(),
            record.args()
        );
        match &self.file {
            // `impl Write for &File`：`&File` 本身就可写，所以 `&self` 已经够用。
            Some(file) => {
                let mut handle = file;
                let _ = handle.write_all(line.as_bytes());
            }
            None => eprint!("{line}"),
        }
    }

    fn flush(&self) {
        if let Some(file) = &self.file {
            let mut handle = file;
            let _ = handle.flush();
        }
    }
}

/// 安装全局 logger。重复调用是幂等的（第二次 `set_boxed_logger` 会失败，忽略即可）。
pub fn init() {
    if log::set_boxed_logger(Box::new(FileLogger {
        file: open_log_file(),
    }))
    .is_err()
    {
        return;
    }
    log::set_max_level(log::LevelFilter::Info);
}

/// `<数据目录>/logs/app.log`；目录或文件建不出来时返回 `None`，届时退回 stderr。
fn open_log_file() -> Option<File> {
    let dir = data_dir()?.join("logs");
    fs::create_dir_all(&dir).ok()?;
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("app.log"))
        .ok()
}
