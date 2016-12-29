use std::{fs, process, io};
use libc;
use libc::{c_int, c_ulong};
use std::os::unix::io::{RawFd, AsRawFd};
use std::io::{Write, Read};

#[cfg(target_os = "linux")]
use libc::c_short;
#[cfg(target_os = "linux")]
use std::path;
#[cfg(target_os = "linux")]
const IFNAMSIZ: usize = 16;
#[cfg(target_os = "linux")]
const IFF_TUN: c_short = 0x0001;
#[cfg(target_os = "linux")]
const IFF_NO_PI: c_short = 0x1000;
#[cfg(target_os = "linux")]
const TUNSETIFF: c_ulong = 0x400454ca;

#[cfg(target_os = "macos")]
use libc::socklen_t;
#[cfg(target_os = "macos")]
use std::mem;
#[cfg(target_os = "macos")]
use std::os::unix::io::FromRawFd;
#[cfg(target_os = "macos")]
use byteorder::{BigEndian, WriteBytesExt};
#[cfg(target_os = "macos")]
const AF_SYS_CONTROL: u16 = 2;
#[cfg(target_os = "macos")]
const AF_SYSTEM: u8 = 32;
#[cfg(target_os = "macos")]
const AF_INET: u8 = 2;
#[cfg(target_os = "macos")]
const PF_SYSTEM: c_int = AF_SYSTEM as c_int;
#[cfg(target_os = "macos")]
const SYSPROTO_CONTROL: c_int = 2;
#[cfg(target_os = "macos")]
const CTLIOCGINFO: c_ulong = 0xc0644e03;
#[cfg(target_os = "macos")]
const UTUN_CONTROL_NAME: &'static str = "com.apple.net.utun_control";

#[cfg(target_os = "linux")]
#[repr(C)]
pub struct ioctl_flags_data {
    pub ifr_name: [u8; IFNAMSIZ],
    pub ifr_flags: c_short,
}

#[cfg(target_os = "macos")]
#[repr(C)]
pub struct ctl_info {
    pub ctl_id: u32,
    pub ctl_name: [u8; 96],
}

#[cfg(target_os = "macos")]
#[repr(C)]
pub struct sockaddr_ctl {
    pub sc_len: u8,
    pub sc_family: u8,
    pub ss_sysaddr: u16,
    pub sc_id: u32,
    pub sc_unit: u32,
    pub sc_reserved: [u32; 5],
}

pub struct Tun {
    handle: fs::File,
    if_name: String,
}

impl AsRawFd for Tun {
    fn as_raw_fd(&self) -> RawFd {
        self.handle.as_raw_fd()
    }
}

impl Tun {
    #[cfg(target_os = "linux")]
    pub fn create(name: u8) -> Tun {
        let path = path::Path::new("/dev/net/tun");
        let file = match fs::OpenOptions::new().read(true).write(true).open(&path) {
            Err(why) => panic!("Couldn't open device '{}': {:?}", path.display(), why),
            Ok(file) => file,
        };

        let mut req = ioctl_flags_data {
            ifr_name: {
                let mut buffer = [0u8; IFNAMSIZ];
                let full_name = format!("tun{}", name);
                buffer[..full_name.len()].clone_from_slice(full_name.as_bytes());
                buffer
            },
            ifr_flags: IFF_TUN | IFF_NO_PI,
        };

        let res = unsafe { libc::ioctl(file.as_raw_fd(), TUNSETIFF, &mut req) }; // TUNSETIFF
        if res < 0 {
            panic!("{}", io::Error::last_os_error());
        }

        let size = req.ifr_name.iter().position(|&r| r == 0).unwrap();
        Tun {
            handle: file,
            if_name: String::from_utf8(req.ifr_name[..size].to_vec()).unwrap(),
        }
    }

    #[cfg(target_os = "macos")]
    pub fn create(name: u8) -> Tun {
        let fd = unsafe { libc::socket(PF_SYSTEM, libc::SOCK_DGRAM, SYSPROTO_CONTROL) };
        if fd < 0 {
            panic!("{}", io::Error::last_os_error());
        }

        let mut info = ctl_info {
            ctl_id: 0,
            ctl_name: {
                let mut buffer = [0u8; 96];
                buffer[..UTUN_CONTROL_NAME.len()].clone_from_slice(UTUN_CONTROL_NAME.as_bytes());
                buffer
            },
        };

        let res = unsafe { libc::ioctl(fd, CTLIOCGINFO, &mut info) };
        if res < 0 {
            unsafe { libc::close(fd) };
            panic!("{}", io::Error::last_os_error());
        }

        let addr = sockaddr_ctl {
            sc_id: info.ctl_id,
            sc_len: mem::size_of::<sockaddr_ctl>() as u8,
            sc_family: AF_SYSTEM,
            ss_sysaddr: AF_SYS_CONTROL,
            sc_unit: name as u32 + 1,
            sc_reserved: [0; 5],
        };

        // If connect() is successful, a tun%d device will be created, where "%d"
        // is our sc_unit-1
        let res = unsafe {
            let addr_ptr = &addr as *const sockaddr_ctl;
            libc::connect(fd,
                          addr_ptr as *const libc::sockaddr,
                          mem::size_of_val(&addr) as socklen_t)
        };
        if res < 0 {
            unsafe { libc::close(fd) };
            panic!("{}", io::Error::last_os_error());
        }

        Tun {
            handle: unsafe { fs::File::from_raw_fd(fd) },
            if_name: format!("utun{}", name),
        }
    }

    pub fn up(&self, self_id: u8) {
        let mut status = if cfg!(target_os = "linux") {
            process::Command::new("ifconfig")
                .arg(self.if_name.clone())
                .arg(format!("10.10.10.{}/24", self_id))
                .status()
                .expect("ifconfig command failed to start")
        } else if cfg!(target_os = "macos") {
            process::Command::new("ifconfig")
                .arg(self.if_name.as_str())
                .arg(format!("10.10.10.{}", self_id))
                .arg("10.10.10.1")
                .status()
                .expect("ifconfig command failed to start")
        } else {
            unimplemented!()
        };

        assert!(status.success());

        status = if cfg!(target_os = "linux") {
            process::Command::new("ifconfig")
                .arg(self.if_name.clone())
                .arg("mtu")
                .arg("1480")
                .arg("up")
                .status()
                .expect("ifconfig command failed to start")
        } else if cfg!(target_os = "macos") {
            process::Command::new("ifconfig")
                .arg(self.if_name.as_str())
                .arg("mtu")
                .arg("1480")
                .arg("up")
                .status()
                .expect("ifconfig command failed to start")
        } else {
            unimplemented!()
        };

        assert!(status.success());
    }

    #[cfg(target_os = "linux")]
    pub fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.handle.write(buf)
    }

    #[cfg(target_os = "linux")]
    pub fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.handle.read(buf)
    }

    #[cfg(target_os = "macos")]
    pub fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut data = vec![];
        {
            data.write_u32::<BigEndian>(AF_INET as u32).unwrap();
            data.write_all(buf).unwrap();
        }
        match self.handle.write(&data) {
            Ok(len) => Ok(if len > 4 { len - 4 } else { 0 }),
            Err(e) => Err(e),
        }
    }

    #[cfg(target_os = "macos")]
    pub fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut data = [0u8; 1600];
        let result = self.handle.read(&mut data);
        match result {
            Ok(len) => {
                buf[..len - 4].clone_from_slice(&data[4..len]);
                Ok(if len > 4 { len - 4 } else { 0 })
            }
            Err(e) => Err(e),
        }
    }
}
