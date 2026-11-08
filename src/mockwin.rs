//! Compile-time stand-in for the `winreg` API so the Windows registry code
//! can be type-checked on Linux (`cargo check --features mockwin`). Never
//! used in a real build.
#![allow(dead_code)]
use std::ffi::OsStr;
use std::io;

pub type HKEY = isize;
pub mod enums {
    pub const HKEY_CURRENT_USER: super::HKEY = 0;
}
pub enum RegDisposition {
    REG_CREATED_NEW_KEY,
}
pub trait ToRegValue {}
impl ToRegValue for String {}
impl ToRegValue for &str {}
pub struct RegKey;
impl RegKey {
    pub const fn predef(_h: HKEY) -> RegKey {
        RegKey
    }
    pub fn create_subkey<P: AsRef<OsStr>>(&self, _p: P) -> io::Result<(RegKey, RegDisposition)> {
        Ok((RegKey, RegDisposition::REG_CREATED_NEW_KEY))
    }
    pub fn set_value<T: ToRegValue, N: AsRef<OsStr>>(&self, _n: N, _v: &T) -> io::Result<()> {
        Ok(())
    }
    pub fn delete_subkey_all<P: AsRef<OsStr>>(&self, _p: P) -> io::Result<()> {
        Ok(())
    }
    pub fn delete_value<N: AsRef<OsStr>>(&self, _n: N) -> io::Result<()> {
        Ok(())
    }
}
