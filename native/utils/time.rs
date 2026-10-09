//! 时间格式化。日志行与错误对话框复制内容都用它，不引入 chrono。

/// RFC3339 UTC 时间戳，秒级精度（`1970-01-01T00:00:00Z` 形如）。
pub(crate) fn rfc3339(secs: u64) -> String {
    let days = secs / 86400;
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    // civil_from_days (Howard Hinnant 算法)
    let z = days as i64 + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mth = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mth <= 2 { y + 1 } else { y };
    format!("{y:04}-{mth:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_known_values() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        // 2026-01-01T00:00:00Z
        assert_eq!(rfc3339(1_767_225_600), "2026-01-01T00:00:00Z");
        // 闰年边界 2024-02-29T12:34:56Z
        assert_eq!(rfc3339(1_709_210_096), "2024-02-29T12:34:56Z");
    }
}
