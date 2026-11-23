//! `GET /fonts/{family}`: a system font this document names, read from the machine
//! that runs the app.
//!
//! A `.pptx` carries typeface names, not glyphs, so a document in 宋体 draws tofu
//! unless the app has that face. It cannot ship in the package — the licence for
//! 宋体 / 微软雅黑 does not allow redistribution — so the app reads the copy the
//! user already has, on the same origin it already serves `/ui/` and `/mcp` from.
//! Nothing crosses the ACowork host, and the app loads only what the document names.

use std::path::PathBuf;

use axum::extract::Path;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;

/// One family the app can serve, with the file names it ships under, in
/// preference order (a newer Windows renames `.ttf` to `.ttc`).
struct Candidate {
    family: &'static str,
    /// The other names this face is asked for by: a `.pptx` built in Word names 宋体,
    /// not `SimSun`, and the same face answers to both.
    aliases: &'static [&'static str],
    files: &'static [&'static str],
}

#[cfg(target_os = "windows")]
const CANDIDATES: &[Candidate] = &[
    Candidate { family: "SimSun", aliases: &["宋体", "NSimSun", "新宋体"], files: &["simsun.ttc", "simsun.ttf"] },
    Candidate { family: "SimHei", aliases: &["黑体", "中易黑体"], files: &["simhei.ttf"] },
    Candidate { family: "KaiTi", aliases: &["楷体", "楷体_GB2312"], files: &["simkai.ttf"] },
    Candidate { family: "FangSong", aliases: &["仿宋", "仿宋_GB2312"], files: &["simfang.ttf"] },
    Candidate { family: "DengXian", aliases: &["等线"], files: &["Deng.ttf", "DengXian.ttf"] },
    Candidate { family: "Microsoft YaHei", aliases: &["微软雅黑", "Microsoft YaHei UI"], files: &["msyh.ttc", "msyh.ttf"] },
    Candidate { family: "Calibri", aliases: &[], files: &["calibri.ttf"] },
    Candidate { family: "Arial", aliases: &[], files: &["arial.ttf"] },
    Candidate { family: "Times New Roman", aliases: &[], files: &["times.ttf"] },
];

// 宋体 is "Songti SC"; 微软雅黑's role is taken by "PingFang SC".
#[cfg(target_os = "macos")]
const CANDIDATES: &[Candidate] = &[
    Candidate { family: "Songti SC", aliases: &["宋体", "SimSun", "STSong"], files: &["Songti.ttc", "Songti.ttf"] },
    Candidate { family: "Heiti SC", aliases: &["黑体", "SimHei"], files: &["STHeiti Medium.ttc", "Heiti.ttc"] },
    Candidate { family: "Kaiti SC", aliases: &["楷体", "KaiTi"], files: &["Kaiti.ttc"] },
    Candidate { family: "PingFang SC", aliases: &["微软雅黑", "Microsoft YaHei", "苹方-简"], files: &["PingFang.ttc"] },
    Candidate { family: "Hiragino Sans GB", aliases: &["冬青黑体"], files: &["Hiragino Sans GB.ttc"] },
    Candidate { family: "Helvetica", aliases: &[], files: &["Helvetica.ttc", "HelveticaNeue.ttc"] },
    Candidate { family: "Arial", aliases: &[], files: &["Arial.ttf"] },
    Candidate { family: "Times New Roman", aliases: &[], files: &["Times New Roman.ttf", "Times.ttc"] },
];

// No 宋体 on a stock Linux: Noto Serif CJK stands in for it, Noto Sans CJK for
// 微软雅黑, Liberation/DejaVu for the Latin roles.
#[cfg(all(unix, not(target_os = "macos")))]
const CANDIDATES: &[Candidate] = &[
    Candidate { family: "Noto Serif CJK SC", aliases: &["宋体", "SimSun"], files: &["NotoSerifCJK-Regular.ttc", "NotoSerifCJKsc-Regular.otf"] },
    Candidate { family: "Noto Sans CJK SC", aliases: &["微软雅黑", "Microsoft YaHei", "黑体", "SimHei"], files: &["NotoSansCJK-Regular.ttc", "NotoSansCJKsc-Regular.otf"] },
    Candidate { family: "Liberation Sans", aliases: &[], files: &["LiberationSans-Regular.ttf"] },
    Candidate { family: "Liberation Serif", aliases: &[], files: &["LiberationSerif-Regular.ttf"] },
    Candidate { family: "Arial", aliases: &[], files: &["LiberationSans-Regular.ttf"] },
    Candidate { family: "Times New Roman", aliases: &[], files: &["LiberationSerif-Regular.ttf"] },
    Candidate { family: "Noto Sans", aliases: &[], files: &["NotoSans-Regular.ttf"] },
    Candidate { family: "Noto Serif", aliases: &[], files: &["NotoSerif-Regular.ttf"] },
    Candidate { family: "DejaVu Sans", aliases: &[], files: &["DejaVuSans.ttf"] },
];

/// The folders this platform keeps fonts in.
fn font_dirs() -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        let mut out = vec![PathBuf::from(windir).join("Fonts")];
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            out.push(PathBuf::from(local).join(r"Microsoft\Windows\Fonts"));
        }
        out
    }
    #[cfg(target_os = "macos")]
    {
        let mut out = vec![PathBuf::from("/System/Library/Fonts"), PathBuf::from("/Library/Fonts")];
        if let Some(home) = std::env::var_os("HOME") {
            out.push(PathBuf::from(home).join("Library/Fonts"));
        }
        out
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let mut out = vec![PathBuf::from("/usr/share/fonts"), PathBuf::from("/usr/local/share/fonts")];
        if let Some(home) = std::env::var_os("HOME") {
            out.push(PathBuf::from(home).join(".fonts"));
            out.push(PathBuf::from(home).join(".local/share/fonts"));
        }
        out
    }
}

/// Find a font file by name under the platform font folders. Distributions nest
/// by vendor (`/usr/share/fonts/truetype/…`), and Linux file names are
/// case-sensitive, so both are handled.
fn find_file(names: &[&'static str]) -> Option<PathBuf> {
    for dir in font_dirs() {
        let nested: Vec<PathBuf> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        for name in names {
            let direct = dir.join(name);
            if direct.is_file() {
                return Some(direct);
            }
            for sub in &nested {
                let candidate = sub.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
                if let Ok(children) = std::fs::read_dir(sub) {
                    for child in children.flatten() {
                        if child.file_name().to_string_lossy().eq_ignore_ascii_case(name) {
                            return Some(child.path());
                        }
                    }
                }
            }
        }
    }
    None
}

/// One face by any name it is asked for, `等线 Light` included: a weight is not a
/// different family, and the file it lives in is the family's.
fn find(family: &str) -> Option<&'static Candidate> {
    let exact = |name: &str| {
        CANDIDATES.iter().find(|c| {
            c.family.eq_ignore_ascii_case(name) || c.aliases.iter().any(|a| a.eq_ignore_ascii_case(name))
        })
    };
    if let Some(c) = exact(family) {
        return Some(c);
    }
    let base = family.rsplit_once(' ').map(|(head, tail)| match tail.to_ascii_lowercase().as_str() {
        "light" | "regular" | "medium" | "bold" | "semilight" | "semibold" | "thin" | "black" => head.trim(),
        _ => family,
    });
    base.filter(|b| *b != family).and_then(exact)
}

/// `GET /fonts/{family}` → the file's base64 (what `font.add` takes), or 404
/// when this machine has no such face.
pub async fn serve(Path(family): Path<String>) -> Response {
    let Some(candidate) = find(family.trim()) else {
        return (StatusCode::NOT_FOUND, "no such family").into_response();
    };
    let Some(path) = find_file(candidate.files) else {
        return (StatusCode::NOT_FOUND, "that family is not installed").into_response();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "could not read the font file").into_response();
    };
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], data).into_response()
}

/// `GET /fonts` → the families this machine can serve, so a caller can check
/// whether the app supplies its own fonts before asking the host for any.
pub async fn list() -> Response {
    let mut out = Vec::new();
    for candidate in CANDIDATES {
        if find_file(candidate.files).is_some() {
            out.push(candidate.family.to_string());
            for alias in candidate.aliases {
                out.push((*alias).to_string());
            }
        }
    }
    axum::Json(out).into_response()
}
