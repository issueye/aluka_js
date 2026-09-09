//! tar.gz 解包：npm tarball（`package/` 首层目录）→ 目标目录安全落盘。
//!
//! 安全不变量（zip-slip 防护）：
//! - 条目路径经 strip 首层组件后归一化，拒绝绝对路径 / `..` 逃逸 / 空路径；
//! - 仅落地普通文件与目录（符号链接 / 硬链接：npm 默认保留包内相对链接，
//!   本实现按安全口径拒链并在返回值登记计数——生态包内自链极罕见）；
//! - 解包目标若已有同名文件则覆盖（npm 重装语义）。

use std::path::{Component, Path, PathBuf};

/// 解包结果统计。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ExtractStats {
    /// 落地文件数
    pub files: usize,
    /// 落地目录数
    pub dirs: usize,
    /// 被拒绝的链接条目数（安全口径）
    pub links_skipped: usize,
}

/// 解包错误。
#[derive(Debug)]
pub enum TarError {
    /// gzip/tar 解码失败
    Decode(String),
    /// 条目路径不安全或非法
    UnsafePath(String),
    /// 文件系统写入失败
    Io(std::io::Error),
}

impl std::fmt::Display for TarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TarError::Decode(m) => write!(f, "tar 解码失败: {m}"),
            TarError::UnsafePath(m) => write!(f, "不安全的 tar 条目路径: {m}"),
            TarError::Io(e) => write!(f, "解包写入失败: {e}"),
        }
    }
}

impl std::error::Error for TarError {}

impl From<std::io::Error> for TarError {
    fn from(e: std::io::Error) -> Self {
        TarError::Io(e)
    }
}

/// 将 `.tgz` 字节流解包到 `dest`（strip 首层 `package/`）。
pub fn extract_tgz(bytes: &[u8], dest: &Path) -> Result<ExtractStats, TarError> {
    // MultiGzDecoder 容忍多成员 gzip 流（个别打包器的输出形态）
    let mut gz = flate2::read::MultiGzDecoder::new(bytes);
    let mut archive = tar::Archive::new(&mut gz);
    // 安全口径：不落盘任何符号链接；时间戳/mode 不还原（Windows 无意义）
    archive.set_preserve_permissions(false);
    archive.set_unpack_xattrs(false);
    let mut stats = ExtractStats::default();
    std::fs::create_dir_all(dest)?;
    for entry in archive
        .entries()
        .map_err(|e| TarError::Decode(e.to_string()))?
    {
        let mut entry = entry.map_err(|e| TarError::Decode(e.to_string()))?;
        let header = entry.header();
        match header.entry_type() {
            tar::EntryType::Regular => {}
            tar::EntryType::Directory => {
                let rel = strip_first(entry.path()?.to_string_lossy().as_ref())?;
                if rel.as_os_str().is_empty() {
                    continue;
                }
                let target = safe_join(dest, &rel)?;
                std::fs::create_dir_all(&target)?;
                stats.dirs += 1;
                continue;
            }
            tar::EntryType::Symlink | tar::EntryType::Link | tar::EntryType::Continuous => {
                stats.links_skipped += 1;
                continue;
            }
            _ => {
                stats.links_skipped += 1;
                continue;
            }
        }
        let rel = strip_first(entry.path()?.to_string_lossy().as_ref())?;
        if rel.as_os_str().is_empty() {
            continue;
        }
        let target = safe_join(dest, &rel)?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::fs::File::create(&target)?;
        std::io::copy(&mut entry, &mut out)?;
        stats.files += 1;
    }
    Ok(stats)
}

/// strip tarball 首层目录组件（npm 约定 `package/`；兼容 scoped 包的非
/// `package` 首层命名——一律 strip 首个组件）。
fn strip_first(path: &str) -> Result<PathBuf, TarError> {
    let p = Path::new(path);
    let mut comps = p.components();
    // 跳过 Windows 盘符/根前缀（tar 内路径不应有，防御性处理）
    comps.next();
    let rel: PathBuf = comps.collect();
    if rel.as_os_str().is_empty() {
        return Ok(rel);
    }
    // 归一化后再校验（`a/./b` 允许；`a/../b` 拒绝）
    for c in rel.components() {
        match c {
            Component::Normal(_) | Component::CurDir => {}
            _ => return Err(TarError::UnsafePath(path.to_owned())),
        }
    }
    Ok(rel)
}

/// 目标路径拼装（`strip_first` 已拒绝 `..`/绝对分量，此处再以 canonicalize
/// 断言落点不逃逸出 `base`，双保险）。
fn safe_join(base: &Path, rel: &Path) -> Result<PathBuf, TarError> {
    let joined = base.join(rel);
    // 父目录此刻可能尚未创建：先对已存在的最深前缀做 canonicalize 比对
    let Ok(base_abs) = base.canonicalize() else {
        return Ok(joined); // base 尚不存在（首装场景）：组件级校验已足够
    };
    let mut probe = joined.clone();
    let mut deepest = None;
    while let Some(parent) = probe.parent() {
        if let Ok(c) = parent.canonicalize() {
            deepest = Some(c);
            break;
        }
        probe = parent.to_path_buf();
    }
    if let Some(deepest) = deepest {
        if !deepest.starts_with(&base_abs) {
            return Err(TarError::UnsafePath(joined.display().to_string()));
        }
    }
    Ok(joined)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// tar 字节流 gzip 包装（npm tarball 形态）。
    fn gzip_bytes(raw: &[u8]) -> Vec<u8> {
        use std::io::Write as _;
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(raw).unwrap();
        enc.finish().unwrap()
    }

    /// 构造一个最小 tar.gz（内存内打包 `package/index.js` + `package/lib/x.js`）。
    fn make_tgz() -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        let add = |b: &mut tar::Builder<Vec<u8>>, name: &str, data: &[u8]| {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            b.append_data(&mut h, name, data).unwrap();
        };
        add(&mut builder, "package/index.js", &b"console.log(1)\n"[..]);
        add(
            &mut builder,
            "package/lib/x.js",
            &b"module.exports=1;\n"[..],
        );
        gzip_bytes(&builder.into_inner().unwrap())
    }

    #[test]
    fn extracts_with_first_component_stripped() {
        let tgz = make_tgz();
        let dest = std::env::temp_dir().join(format!("aluka_npm_t_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dest);
        let stats = extract_tgz(&tgz, &dest).unwrap();
        assert_eq!(stats.files, 2);
        assert!(dest.join("index.js").is_file());
        assert!(dest.join("lib/x.js").is_file());
        let _ = std::fs::remove_dir_all(&dest);
    }

    /// 手工构造包含 `package/../evil.js` 条目的原始 tar 字节
    /// （tar::Builder 构建期即拒绝 `..`，恶意夹具只能手工拼装）。
    fn evil_tar() -> Vec<u8> {
        let mut buf = vec![0u8; 512];
        let name = b"package/../evil.js";
        buf[..name.len()].copy_from_slice(name);
        buf[100..107].copy_from_slice(b"0000644"); // mode
        buf[108..115].copy_from_slice(b"0000000"); // uid
        buf[116..123].copy_from_slice(b"0000000"); // gid
        buf[124..135].copy_from_slice(b"00000000003"); // size = 3
        buf[136..147].copy_from_slice(b"00000000000"); // mtime
        buf[156] = b'0'; // typeflag: regular
        buf[257..262].copy_from_slice(b"ustar"); // magic
        buf[263..265].copy_from_slice(b"00"); // version
        // checksum：字段区先置空格占位求和，再写六位八进制 + NUL + 空格
        buf[148..156].fill(b' ');
        let sum: u64 = buf.iter().map(|&b| u64::from(b)).sum();
        let ck = format!("{sum:06o}");
        buf[148..154].copy_from_slice(ck.as_bytes());
        buf[154] = 0;
        buf[155] = b' ';
        // 数据块（3 字节 + 补齐 512）
        let mut data = vec![b'b', b'a', b'd'];
        data.resize(512, 0);
        buf.extend_from_slice(&data);
        buf
    }

    #[test]
    fn rejects_escape_entries() {
        let tgz = gzip_bytes(&evil_tar());
        let dest = std::env::temp_dir().join(format!("aluka_npm_e_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dest);
        let err = extract_tgz(&tgz, &dest).unwrap_err();
        assert!(
            matches!(err, TarError::UnsafePath(_)),
            "应拒绝 .. 逃逸: {err}"
        );
        assert!(
            !dest.join("evil.js").exists() && !dest.parent().unwrap().join("evil.js").exists(),
            "逃逸文件不得落盘"
        );
        let _ = std::fs::remove_dir_all(&dest);
    }

    #[test]
    fn corrupt_gzip_reports_decode_error() {
        let dest = std::env::temp_dir().join(format!("aluka_npm_c_{}", std::process::id()));
        let err = extract_tgz(b"not a gzip", &dest).unwrap_err();
        assert!(matches!(err, TarError::Decode(_)));
    }
}
