//! Node.js `path` module — pure path manipulation, POSIX-style.

use crate::value::{TsVal, UNDEFINED, TsArray};
use super::{new_string, val_to_string, val_to_i32};
use std::path::{Path, PathBuf};

unsafe fn arr_to_strs(arr: TsVal) -> Vec<String> {
    if !arr.is_ptr() { return vec![]; }
    let a = &*(arr.as_ptr() as *const TsArray);
    a.elements.iter().filter_map(|&v| val_to_string(v)).collect()
}

fn normalize_path(p: &Path) -> String {
    let s = p.to_string_lossy();
    let leading = s.starts_with('/');
    let mut parts: Vec<&str> = vec![];
    for c in p.components() {
        match c {
            std::path::Component::RootDir => {}
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => { parts.pop(); }
            std::path::Component::Normal(s) => parts.push(s.to_str().unwrap_or("")),
            std::path::Component::Prefix(_) => {}
        }
    }
    let joined = parts.join("/");
    if leading { format!("/{}", joined) }
    else if joined.is_empty() { ".".to_string() }
    else { joined }
}

#[no_mangle]
pub unsafe extern "C" fn ts_path_join(parts: TsVal) -> TsVal {
    let strs = arr_to_strs(parts);
    if strs.is_empty() { return new_string("."); }
    let mut buf = PathBuf::new();
    for s in &strs { buf.push(s.as_str()); }
    new_string(&normalize_path(&buf))
}

#[no_mangle]
pub unsafe extern "C" fn ts_path_resolve(parts: TsVal) -> TsVal {
    let strs = arr_to_strs(parts);
    let mut buf = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    for s in &strs {
        if s.starts_with('/') { buf = PathBuf::from(s); }
        else { buf.push(s); }
    }
    new_string(&normalize_path(&buf))
}

#[no_mangle]
pub unsafe extern "C" fn ts_path_dirname(p: TsVal) -> TsVal {
    let s = match val_to_string(p) { Some(s) => s, None => return new_string(".") };
    let parent = Path::new(&s).parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".to_string());
    new_string(if parent.is_empty() { "/" } else { &parent })
}

#[no_mangle]
pub unsafe extern "C" fn ts_path_basename(p: TsVal, ext: TsVal) -> TsVal {
    let s = match val_to_string(p) { Some(s) => s, None => return new_string("") };
    let base = Path::new(&s).file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let result = if let Some(ext_str) = val_to_string(ext) {
        if !ext_str.is_empty() && base.ends_with(&ext_str) {
            base[..base.len()-ext_str.len()].to_string()
        } else { base }
    } else { base };
    new_string(&result)
}

#[no_mangle]
pub unsafe extern "C" fn ts_path_extname(p: TsVal) -> TsVal {
    let s = match val_to_string(p) { Some(s) => s, None => return new_string("") };
    let ext = Path::new(&s).extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    new_string(&ext)
}

#[no_mangle]
pub unsafe extern "C" fn ts_path_normalize(p: TsVal) -> TsVal {
    let s = match val_to_string(p) { Some(s) => s, None => return new_string(".") };
    new_string(&normalize_path(Path::new(&s)))
}

#[no_mangle]
pub unsafe extern "C" fn ts_path_is_absolute(p: TsVal) -> TsVal {
    let s = match val_to_string(p) { Some(s) => s, None => return TsVal::from_bool(false) };
    TsVal::from_bool(Path::new(&s).is_absolute())
}

#[no_mangle]
pub unsafe extern "C" fn ts_path_relative(from: TsVal, to: TsVal) -> TsVal {
    let from_s = match val_to_string(from) { Some(s) => s, None => return new_string("") };
    let to_s   = match val_to_string(to)   { Some(s) => s, None => return new_string("") };
    if to_s.starts_with(&from_s) {
        let rel = to_s[from_s.len()..].trim_start_matches('/');
        return new_string(if rel.is_empty() { "." } else { rel });
    }
    new_string(&to_s)
}
