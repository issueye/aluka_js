//! Node `lib/path.js` 的逐字移植：`basename` / `extname` 两个平台共用算法。
//!
//! 为什么不用 Go 的 `path.Base`/`path.Ext`：本项目的唯一 oracle 是
//! **Node.js 22 LTS**，而 Go 版在可观测输出上与 Node 有系统性差异——
//! Go 的 `Base` 先 `Clean`（`basename('/')` 得 `'/'`、`basename('')` 得 `'.'`），
//! Node 返回**原串切片**（分别为 `''`、`''`）；Go 的 `Ext` 把 `'..'` 判为
//! 扩展名 `'.'`，Node 的 `preDotState` 状态机判为 `''`。凡是真实包可观测
//! 的路径值都必须以 Node 为准。
//!
//! 分隔符差异通过 `sep_is_sep` / `root_sep` 参数注入：POSIX 只认 `/`，
//! win32 同时认 `/` 与 `\\`。

/// 分隔符谓词。
pub(crate) type SepFn = fn(u8) -> bool;

/// POSIX：仅 `/`。
pub(crate) fn is_posix_sep(c: u8) -> bool {
    c == b'/'
}

/// win32：`/` 与 `\\`（Node `isPathSeparator`）。
pub(crate) fn is_win_sep(c: u8) -> bool {
    c == b'/' || c == b'\\'
}

/// Node `basename(path[, suffix])`（`lib/path.js` 的 posix/win32 共用体）。
///
/// - `''` / 全分隔符 → `''`
/// - `suffix` 逐字符从尾部比对，不匹配则整体回退为该路径分量
/// - `suffix === path` → `''`
///
/// `device_root` 为 true 时按 win32 语义跳过开头的设备根（`C:` 两字节），
/// 避免把其后紧跟的分隔符误判为可忽略的尾部分隔符
///（`basename('C:a') === 'a'`、`basename('C:\\') === ''`）。
pub(crate) fn node_basename(
    path: &str,
    suffix: Option<&str>,
    is_sep: SepFn,
    device_root: bool,
) -> String {
    let b = path.as_bytes();
    let len = b.len();
    let mut start = 0usize;
    let mut end: isize = -1;
    let mut matched_slash = true;

    if device_root && len >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        start = 2;
    }

    if let Some(suf) = suffix {
        let s = suf.as_bytes();
        if !s.is_empty() && s.len() <= len {
            if suf == path {
                return String::new();
            }
            let mut ext_idx: isize = s.len() as isize - 1;
            let mut first_non_slash_end: isize = -1;
            let mut i = len as isize - 1;
            while i >= start as isize {
                let code = b[i as usize];
                if is_sep(code) {
                    // 尾部连续分隔符组里的分隔符不构成边界
                    if !matched_slash {
                        start = i as usize + 1;
                        break;
                    }
                } else {
                    if first_non_slash_end == -1 {
                        matched_slash = false;
                        first_non_slash_end = i + 1;
                    }
                    if ext_idx >= 0 {
                        if code == s[ext_idx as usize] {
                            ext_idx -= 1;
                            if ext_idx == -1 {
                                // 扩展名整体匹配：路径分量到此为止
                                end = i;
                            }
                        } else {
                            // 不匹配：结果为整个路径分量
                            ext_idx = -1;
                            end = first_non_slash_end;
                        }
                    }
                }
                i -= 1;
            }
            if start as isize == end {
                end = first_non_slash_end;
            } else if end == -1 {
                end = len as isize;
            }
            return path[start..end as usize].to_owned();
        }
    }

    let mut i = len as isize - 1;
    while i >= start as isize {
        let code = b[i as usize];
        if is_sep(code) {
            if !matched_slash {
                start = i as usize + 1;
                break;
            }
        } else if end == -1 {
            matched_slash = false;
            end = i + 1;
        }
        i -= 1;
    }
    if end == -1 {
        return String::new();
    }
    path[start..end as usize].to_owned()
}

/// Node `extname(path)`（`preDotState` 状态机，posix/win32 共用）。
///
/// 返回 `''` 的情形：无点、无路径分量、点前紧邻非点字符（`'a.'`）、
/// 或分量恰为 `'..'`（`preDotState === 1 && startDot === end - 1 &&
/// startDot === startPart + 1`）。
pub(crate) fn node_extname(path: &str, is_sep: SepFn) -> String {
    let b = path.as_bytes();
    let mut start_dot: isize = -1;
    let mut start_part: isize = 0;
    let mut end: isize = -1;
    let mut matched_slash = true;
    let mut pre_dot_state: i32 = 0;

    let mut i = b.len() as isize - 1;
    while i >= 0 {
        let ch = b[i as usize];
        if is_sep(ch) {
            if !matched_slash {
                start_part = i + 1;
                break;
            }
            i -= 1;
            continue;
        }
        if end == -1 {
            matched_slash = false;
            end = i + 1;
        }
        if ch == b'.' {
            if start_dot == -1 {
                start_dot = i;
            } else if pre_dot_state != 1 {
                pre_dot_state = 1;
            }
        } else if start_dot != -1 {
            pre_dot_state = -1;
        }
        i -= 1;
    }

    if start_dot == -1
        || end == -1
        || pre_dot_state == 0
        || (pre_dot_state == 1 && start_dot == end - 1 && start_dot == start_part + 1)
    {
        return String::new();
    }
    path[start_dot as usize..end as usize].to_owned()
}
