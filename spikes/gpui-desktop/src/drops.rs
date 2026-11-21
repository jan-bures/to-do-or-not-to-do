//! Drop logging: stdout + drops.txt. `sniff` replaces GPUI's IDropTarget with one that logs every
//! clipboard format offered by the drag source (CF_HDROP, FileGroupDescriptorW, ...).
use std::io::Write;
use windows::{
    Win32::{
        Foundation::*,
        System::{Com::*, DataExchange::*, Memory::*, Ole::*, SystemServices::MODIFIERKEYS_FLAGS},
        UI::Shell::*,
    },
    core::*,
};

pub fn drop_log(line: impl AsRef<str>) {
    let line = line.as_ref();
    println!("[drop] {line}");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("drops.txt") {
        let _ = writeln!(f, "{line}");
    }
}

fn fmt_name(cf: u16) -> String {
    match cf {
        1 => "CF_TEXT".into(),
        2 => "CF_BITMAP".into(),
        13 => "CF_UNICODETEXT".into(),
        15 => "CF_HDROP".into(),
        16 => "CF_LOCALE".into(),
        17 => "CF_DIBV5".into(),
        _ => {
            let mut b = [0u16; 128];
            let n = unsafe { GetClipboardFormatNameW(cf as u32, &mut b) };
            if n > 0 { String::from_utf16_lossy(&b[..n as usize]) } else { format!("cf{cf}") }
        }
    }
}

fn describe(tag: &str, d: &IDataObject) {
    drop_log(format!("--- {tag}: formats offered by drag source ---"));
    unsafe {
        let Ok(en) = d.EnumFormatEtc(DATADIR_GET.0 as u32) else {
            drop_log("EnumFormatEtc failed");
            return;
        };
        loop {
            let mut f = [FORMATETC::default()];
            let mut got = 0u32;
            if en.Next(&mut f, Some(&mut got)).0 != 0 || got == 0 {
                break;
            }
            drop_log(format!("  format {:>5} {:<28} tymed=0x{:x} lindex={}", f[0].cfFormat, fmt_name(f[0].cfFormat), f[0].tymed, f[0].lindex));
        }
    }
}

fn reg(name: PCWSTR) -> u16 {
    unsafe { RegisterClipboardFormatW(name) as u16 }
}

fn extract(d: &IDataObject) {
    unsafe {
        let hdrop = FORMATETC { cfFormat: 15, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
        if d.QueryGetData(&hdrop) == S_OK {
            drop_log("CF_HDROP present -> GPUI would deliver ExternalPaths");
        } else {
            drop_log("CF_HDROP NOT present -> GPUI's Windows drop handler would ignore this drag (DROPEFFECT_NONE on enter, no ExternalPaths)");
        }
        let fgd = reg(w!("FileGroupDescriptorW"));
        let fc = reg(w!("FileContents"));
        let f = FORMATETC { cfFormat: fgd, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
        if d.QueryGetData(&f) != S_OK {
            drop_log("FileGroupDescriptorW not present");
            return;
        }
        let Ok(mut m) = d.GetData(&f) else { return };
        let h = m.u.hGlobal;
        let p = GlobalLock(h) as *const FILEGROUPDESCRIPTORW;
        if p.is_null() {
            return;
        }
        let n = (*p).cItems as usize;
        drop_log(format!("FileGroupDescriptorW: {n} virtual file(s)"));
        let mut names = vec![];
        for i in 0..n {
            let fd: FILEDESCRIPTORW = std::ptr::read_unaligned((*p).fgd.as_ptr().add(i));
            let cname = fd.cFileName;
            let flags = fd.dwFlags;
            let name_len = cname.iter().position(|c| *c == 0).unwrap_or(260);
            let name = String::from_utf16_lossy(&cname[..name_len]);
            let size = ((fd.nFileSizeHigh as u64) << 32) | fd.nFileSizeLow as u64;
            drop_log(format!("  [{i}] name={name:?} declared_size={size} flags=0x{:x}", flags));
            names.push(i);
        }
        let _ = GlobalUnlock(h);
        ReleaseStgMedium(&mut m);
        for i in names {
            let f = FORMATETC { cfFormat: fc, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: i as i32, tymed: (TYMED_ISTREAM.0 | TYMED_HGLOBAL.0) as u32 };
            match d.GetData(&f) {
                Ok(mut m) => {
                    if m.tymed == TYMED_ISTREAM.0 as u32 {
                        if let Some(s) = (*m.u.pstm).as_ref() {
                            let mut total = 0usize;
                            let mut head = [0u8; 16];
                            let mut buf = vec![0u8; 64 * 1024];
                            loop {
                                let mut read = 0u32;
                                let _ = s.Read(buf.as_mut_ptr() as *mut _, buf.len() as u32, Some(&mut read));
                                if read == 0 {
                                    break;
                                }
                                if total < 16 {
                                    let k = (16 - total).min(read as usize);
                                    head[total..total + k].copy_from_slice(&buf[..k]);
                                }
                                total += read as usize;
                            }
                            drop_log(format!("  FileContents[{i}] via IStream: {total} bytes, first16={}", head.iter().map(|b| format!("{b:02x}")).collect::<String>()));
                        }
                    } else {
                        drop_log(format!("  FileContents[{i}] tymed=0x{:x} size={}", m.tymed, GlobalSize(m.u.hGlobal)));
                    }
                    ReleaseStgMedium(&mut m);
                }
                Err(e) => drop_log(format!("  FileContents[{i}] GetData failed: {e}")),
            }
        }
    }
}

#[implement(IDropTarget)]
struct Sniff;

#[allow(non_snake_case)]
impl IDropTarget_Impl for Sniff_Impl {
    fn DragEnter(&self, pdataobj: Ref<IDataObject>, _k: MODIFIERKEYS_FLAGS, _pt: &POINTL, eff: *mut DROPEFFECT) -> Result<()> {
        unsafe { *eff = DROPEFFECT_COPY };
        if let Ok(d) = pdataobj.ok() {
            describe("DragEnter", d);
        }
        Ok(())
    }
    fn DragOver(&self, _k: MODIFIERKEYS_FLAGS, _pt: &POINTL, eff: *mut DROPEFFECT) -> Result<()> {
        unsafe { *eff = DROPEFFECT_COPY };
        Ok(())
    }
    fn DragLeave(&self) -> Result<()> {
        Ok(())
    }
    fn Drop(&self, pdataobj: Ref<IDataObject>, _k: MODIFIERKEYS_FLAGS, _pt: &POINTL, eff: *mut DROPEFFECT) -> Result<()> {
        unsafe { *eff = DROPEFFECT_COPY };
        if let Ok(d) = pdataobj.ok() {
            describe("Drop", d);
            extract(d);
        }
        Ok(())
    }
}

pub fn install_sniffer(hwnd: isize) {
    unsafe {
        let h = HWND(hwnd as *mut _);
        let _ = RevokeDragDrop(h);
        let t: IDropTarget = Sniff.into();
        match RegisterDragDrop(h, &t) {
            Ok(_) => drop_log("sniffer IDropTarget registered (GPUI's drop handler replaced)"),
            Err(e) => drop_log(format!("RegisterDragDrop failed: {e}")),
        }
        std::mem::forget(t);
    }
}
