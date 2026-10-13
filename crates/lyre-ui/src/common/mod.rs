//! 与业务无关的自绘基础件（不认识 `Track`、`Playlist`、`PlaybackEntity`）。

pub mod button;
pub mod cover;
pub mod gradient;
pub mod menu;
pub mod scroll;
pub mod section;
pub mod toast;
pub mod tooltip;

pub fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    // 不写 `use super::*`：`crate::platform::*` 带进来的 gpui `test` 宏会遮蔽内置 `#[test]`。
    #[test]
    fn thousands_groups_by_three() {
        assert_eq!(super::thousands(0), "0");
        assert_eq!(super::thousands(999), "999");
        assert_eq!(super::thousands(1000), "1,000");
        assert_eq!(super::thousands(1284), "1,284");
        assert_eq!(super::thousands(1_000_000), "1,000,000");
    }
}
