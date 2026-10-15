//! 描边类图标的 SVG 数据，从 Lucide v0.544.0（`lucide-static`，ISC）批量生成，手工改动会在下次生成时被覆盖。

use crate::icon::Glyph;

/// 外壳属性统一在这里给，避免上游 SVG 各自的 `stroke-width` 让线宽不一致。
macro_rules! wrap {
    ($($inner:literal),+ $(,)?) => {
        concat!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" "#,
            r#"stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">"#,
            $($inner),+,
            "</svg>"
        )
    };
}

/// [`Glyph`] 里的填充类图标不走这里，返回空串。
pub fn stroke_svg(glyph: Glyph) -> &'static str {
    match glyph {
        Glyph::Search => wrap!(
            r#"  <path d="m21 21-4.34-4.34" />"#,
            r#"  <circle cx="11" cy="11" r="8" />"#,
        ),
        Glyph::Shuffle => wrap!(
            r#"  <path d="m18 14 4 4-4 4" />"#,
            r#"  <path d="m18 2 4 4-4 4" />"#,
            r#"  <path d="M2 18h1.973a4 4 0 0 0 3.3-1.7l5.454-8.6a4 4 0 0 1 3.3-1.7H22" />"#,
            r#"  <path d="M2 6h1.972a4 4 0 0 1 3.6 2.2" />"#,
            r#"  <path d="M22 18h-6.041a4 4 0 0 1-3.3-1.8l-.359-.45" />"#,
        ),
        Glyph::RepeatAll => wrap!(
            r#"  <path d="m17 2 4 4-4 4" />"#,
            r#"  <path d="M3 11v-1a4 4 0 0 1 4-4h14" />"#,
            r#"  <path d="m7 22-4-4 4-4" />"#,
            r#"  <path d="M21 13v1a4 4 0 0 1-4 4H3" />"#,
        ),
        Glyph::RepeatOne => wrap!(
            r#"  <path d="m17 2 4 4-4 4" />"#,
            r#"  <path d="M3 11v-1a4 4 0 0 1 4-4h14" />"#,
            r#"  <path d="m7 22-4-4 4-4" />"#,
            r#"  <path d="M21 13v1a4 4 0 0 1-4 4H3" />"#,
            r#"  <path d="M11 10h1v4" />"#,
        ),
        Glyph::List => wrap!(
            r#"  <path d="M3 5h.01" />"#,
            r#"  <path d="M3 12h.01" />"#,
            r#"  <path d="M3 19h.01" />"#,
            r#"  <path d="M8 5h13" />"#,
            r#"  <path d="M8 12h13" />"#,
            r#"  <path d="M8 19h13" />"#,
        ),
        Glyph::Grid => wrap!(
            r#"  <path d="M12 3v18" />"#,
            r#"  <path d="M3 12h18" />"#,
            r#"  <rect x="3" y="3" width="18" height="18" rx="2" />"#,
        ),
        Glyph::Queue => wrap!(
            r#"  <path d="M16 5H3" />"#,
            r#"  <path d="M11 12H3" />"#,
            r#"  <path d="M11 19H3" />"#,
            r#"  <path d="M21 16V5" />"#,
            r#"  <circle cx="18" cy="16" r="3" />"#,
        ),
        Glyph::Folder => wrap!(
            r#"  <path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z" />"#,
        ),
        Glyph::Album => wrap!(
            r#"  <circle cx="12" cy="12" r="10" />"#,
            r#"  <path d="M6 12c0-1.7.7-3.2 1.8-4.2" />"#,
            r#"  <circle cx="12" cy="12" r="2" />"#,
            r#"  <path d="M18 12c0 1.7-.7 3.2-1.8 4.2" />"#,
        ),
        Glyph::Artist => wrap!(
            r#"  <path d="m11 7.601-5.994 8.19a1 1 0 0 0 .1 1.298l.817.818a1 1 0 0 0 1.314.087L15.09 12" />"#,
            r#"  <path d="M16.5 21.174C15.5 20.5 14.372 20 13 20c-2.058 0-3.928 2.356-6 2-2.072-.356-2.775-3.369-1.5-4.5" />"#,
            r#"  <circle cx="16" cy="7" r="5" />"#,
        ),
        Glyph::More => wrap!(
            r#"  <circle cx="12" cy="12" r="1" />"#,
            r#"  <circle cx="19" cy="12" r="1" />"#,
            r#"  <circle cx="5" cy="12" r="1" />"#,
        ),
        Glyph::Heart => wrap!(
            r#"  <path d="M2 9.5a5.5 5.5 0 0 1 9.591-3.676.56.56 0 0 0 .818 0A5.49 5.49 0 0 1 22 9.5c0 2.29-1.5 4-3 5.5l-5.492 5.313a2 2 0 0 1-3 .019L5 15c-1.5-1.5-3-3.2-3-5.5" />"#,
        ),
        Glyph::ChevronDown => wrap!(r#"  <path d="m6 9 6 6 6-6" />"#,),
        Glyph::ChevronRight => wrap!(r#"  <path d="m9 18 6-6-6-6" />"#,),
        // 以下几项是手工加的，重新生成时会被覆盖掉。
        Glyph::ArrowLeft => wrap!(
            r#"  <path d="m12 19-7-7 7-7" />"#,
            r#"  <path d="M19 12H5" />"#,
        ),
        Glyph::ArrowRight => wrap!(
            r#"  <path d="M5 12h14" />"#,
            r#"  <path d="m12 5 7 7-7 7" />"#,
        ),
        Glyph::Home => wrap!(
            r#"  <path d="M15 21v-8a1 1 0 0 0-1-1h-4a1 1 0 0 0-1 1v8" />"#,
            r#"  <path d="M3 10a2 2 0 0 1 .709-1.528l7-6a2 2 0 0 1 2.582 0l7 6A2 2 0 0 1 21 10v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />"#,
        ),
        Glyph::PanelLeft => wrap!(
            r#"  <rect width="18" height="18" x="3" y="3" rx="2" />"#,
            r#"  <path d="M9 3v18" />"#,
        ),
        Glyph::PanelLeftOpen => wrap!(
            r#"  <rect width="18" height="18" x="3" y="3" rx="2" />"#,
            r#"  <path d="M9 3v18" />"#,
            r#"  <path d="m14 9 3 3-3 3" />"#,
        ),
        Glyph::VolumeHigh => wrap!(
            r#"  <path d="M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298z" />"#,
            r#"  <path d="M16 9a5 5 0 0 1 0 6" />"#,
            r#"  <path d="M19.364 18.364a9 9 0 0 0 0-12.728" />"#,
        ),
        Glyph::VolumeLow => wrap!(
            r#"  <path d="M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298z" />"#,
            r#"  <path d="M16 9a5 5 0 0 1 0 6" />"#,
        ),
        Glyph::VolumeMuted => wrap!(
            r#"  <path d="M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298z" />"#,
            r#"  <line x1="22" x2="16" y1="9" y2="15" />"#,
            r#"  <line x1="16" x2="22" y1="9" y2="15" />"#,
        ),
        Glyph::Check => wrap!(r#"  <path d="M20 6 9 17l-5-5" />"#,),
        Glyph::Undo => wrap!(
            r#"  <path d="M9 14 4 9l5-5" />"#,
            r#"  <path d="M4 9h10.5a5.5 5.5 0 0 1 5.5 5.5a5.5 5.5 0 0 1-5.5 5.5H11" />"#,
        ),
        Glyph::Clock => wrap!(
            r#"  <path d="M12 6v6l4 2" />"#,
            r#"  <circle cx="12" cy="12" r="10" />"#,
        ),
        Glyph::Trash => wrap!(
            r#"  <path d="M10 11v6" />"#,
            r#"  <path d="M14 11v6" />"#,
            r#"  <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6" />"#,
            r#"  <path d="M3 6h18" />"#,
            r#"  <path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />"#,
        ),
        Glyph::Import => wrap!(
            r#"  <path d="M12 3v12" />"#,
            r#"  <path d="m17 8-5-5-5 5" />"#,
            r#"  <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />"#,
        ),
        Glyph::Settings => wrap!(
            r#"  <path d="M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915" />"#,
            r#"  <circle cx="12" cy="12" r="3" />"#,
        ),
        Glyph::Cross => wrap!(
            r#"  <path d="M18 6 6 18" />"#,
            r#"  <path d="m6 6 12 12" />"#,
        ),
        Glyph::Plus => wrap!(r#"  <path d="M5 12h14" />"#, r#"  <path d="M12 5v14" />"#,),
        Glyph::Sort => wrap!(
            r#"  <path d="m3 16 4 4 4-4" />"#,
            r#"  <path d="M7 20V4" />"#,
            r#"  <path d="m21 8-4-4-4 4" />"#,
            r#"  <path d="M17 4v16" />"#,
        ),
        Glyph::Music => wrap!(
            r#"  <path d="M9 18V5l12-2v13" />"#,
            r#"  <circle cx="6" cy="18" r="3" />"#,
            r#"  <circle cx="18" cy="16" r="3" />"#,
        ),
        // 顶栏右上用户区，数据源：Lucide bell / user
        Glyph::Bell => wrap!(
            r#"  <path d="M10.268 21a2 2 0 0 0 3.464 0" />"#,
            r#"  <path d="M3.262 15.326A1 1 0 0 0 4 17h16a1 1 0 0 0 .74-1.673C19.41 13.956 18 12.499 18 8A6 6 0 0 0 6 8c0 4.499-1.411 5.956-2.738 7.326" />"#,
        ),
        Glyph::User => wrap!(
            r#"  <path d="M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2" />"#,
            r#"  <circle cx="12" cy="7" r="4" />"#,
        ),
        // 底栏入口，数据源：Lucide mic / cast / maximize / panel-right
        Glyph::Lyrics => wrap!(
            r#"  <path d="M12 19v3" />"#,
            r#"  <path d="M19 10v2a7 7 0 0 1-14 0v-2" />"#,
            r#"  <rect x="9" y="2" width="6" height="13" rx="3" />"#,
        ),
        Glyph::Devices => wrap!(
            r#"  <path d="M2 8V6a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2h-6" />"#,
            r#"  <path d="M2 12a9 9 0 0 1 8 8" />"#,
            r#"  <path d="M2 16a5 5 0 0 1 4 4" />"#,
            r#"  <path d="M2 20h.01" />"#,
        ),
        Glyph::Fullscreen => wrap!(
            r#"  <path d="M8 3H5a2 2 0 0 0-2 2v3" />"#,
            r#"  <path d="M21 8V5a2 2 0 0 0-2-2h-3" />"#,
            r#"  <path d="M3 16v3a2 2 0 0 0 2 2h3" />"#,
            r#"  <path d="M16 21h3a2 2 0 0 0 2-2v-3" />"#,
        ),
        Glyph::PanelRight => wrap!(
            r#"  <rect width="18" height="18" x="3" y="3" rx="2" />"#,
            r#"  <path d="M15 3v18" />"#,
        ),
        _ => "",
    }
}
