use std::{
    env,
    ffi::CString,
    fs,
    path::{Path, PathBuf},
};

use log::{debug, error, info, warn};

use nix::{
    fcntl::{open, OFlag},
    mount::{mount, MsFlags},
    sched::{unshare, CloneFlags},
    sys::stat::Mode,
    unistd::{chroot, close, execve, Gid, Uid},
};

mod id_map;
use id_map::*;

struct AppRun {
    mount_dir: PathBuf,
    nix_dir: PathBuf,
    entrypoint: PathBuf,
    debug: bool,
}

fn test_openable() -> Result<bool, nix::Error> {
    const TEST_FILE: &str = "/dev/megaraid_sas_ioctl_node";
    let test_file = PathBuf::from(TEST_FILE);

    match open(&test_file, OFlag::O_RDONLY, Mode::empty()) {
        Ok(fd) => {
            close(fd)?;
            debug!("Openable test - Success");
            Ok(true)
        }
        Err(e) => {
            error!("Openable test - Error: {e}");
            Ok(false)
        }
    }
}

impl AppRun {
    fn exec(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.mounts()?;
        self.chroot()?;

        if self.debug {
            test_openable()?;
        }

        // Execute a shell
        // https://stackoverflow.com/questions/38948669/whats-the-most-direct-way-to-convert-a-path-to-a-c-char
        let cmd = CString::new(self.entrypoint.as_os_str().to_str().unwrap())?;
        let args = std::env::args();
        let args: Vec<CString> = args.map(|s| CString::new(s).unwrap()).collect();
        info!("Executing entrypoint with {:?}", args);
        execve(&cmd, &args, &[CString::new("TERM=xterm-256color")?])?;

        Ok(())
    }

    fn write_id_maps(&self) -> Result<(), std::io::Error> {
        let uid = Uid::current();
        let gid = Gid::current();
        info!("uid: {}, gid: {}", uid, gid);

        let uid_map: UidMap = UidMap {
            inside_id: uid,
            outside_id: uid,
            count: 1,
        };
        let gid_map = GidMap {
            inside_id: gid,
            outside_id: gid,
            count: 1,
        };
        std::fs::write(PathBuf::from("/proc/self/uid_map"), uid_map.to_string())?;
        info!("Wrote uid_map");
        std::fs::write(PathBuf::from("/proc/self/setgroups"), "deny")?;
        std::fs::write(PathBuf::from("/proc/self/gid_map"), gid_map.to_string())?;
        info!("Wrote gid_map");

        Ok(())
    }

    /// Create a new mount namespace, bind mount everything from / into the mount_dir,
    /// and bind mount /nix from self.nix_to_mount
    fn mounts(&self) -> Result<(), std::io::Error> {
        // Create a new mount namespace
        info!("Creating new mount namespace");
        let clone_flags = CloneFlags::CLONE_NEWNS;
        unshare(clone_flags)?;
        if clone_flags.contains(CloneFlags::CLONE_NEWUSER) {
            info!("Created new user namespace");
            self.write_id_maps()?;
        }

        // Mark all mount points as slave
        info!("Mounting / as rslave");
        mount(
            None::<&str>,
            "/",
            None::<&str>,
            MsFlags::MS_SLAVE | MsFlags::MS_REC,
            None::<&str>,
        )?;
        if self.debug {
            // Read and print mountinfo
            let mountinfo = std::fs::read_to_string("/proc/self/mountinfo")?;
            debug!("mountinfo: {mountinfo}");
        }

        // Mount a tmpfs
        info!("Mounting tmpfs");
        mount(
            Some("tmpfs"),
            &self.mount_dir,
            Some("tmpfs"),
            MsFlags::MS_NOSUID,
            Some("mode=755"),
        )?;

        // https://www.kernel.org/doc/Documentation/filesystems/sharedsubtree.txt
        let mount_flags = 
            // Recursively bind mount
            MsFlags::MS_BIND | MsFlags::MS_REC |
            // Make this mount point a slave so that mounts in the container don't propagate to the host
            MsFlags::MS_SLAVE |
            MsFlags::MS_UNBINDABLE;

        // Copy over root directories
        let files = fs::read_dir("/")?;
        for file in files {
            let path = file?.path();
            let path_name = path.file_name().unwrap();
            let mount_path = self.mount_dir.join(path_name);

            if path_name == "nix" {
                continue;
            }

            if !path.exists() {
                warn!("Skipping non-existent path {:?}", path);
                continue;
            }

            let mount_result = if path.is_dir() {
                // Create bind mount
                info!("Creating bind mount for {path_name:?}");
                fs::create_dir_all(&mount_path)?;
                mount::<_, _, Path, Path>(Some(&path), &mount_path, None, mount_flags, None)
            } else {
                // Create a file and bind mount it
                info!("Creating bind mount for {path_name:?}");
                fs::write(&mount_path, "")?;
                mount::<_, _, Path, Path>(Some(&path), &mount_path, None, mount_flags, None)
            };

            if let Err(e) = mount_result {
                warn!("Failed to mount {path_name:?}: {e:?}");
            }
        }

        // Bind mount /nix from self.nix_to_mount
        let mount_path = self.mount_dir.join("nix");
        fs::create_dir_all(&mount_path)?;
        info!("Creating bind mount for /nix from {:?}", self.nix_dir);
        mount(
            Some(&self.nix_dir),
            &mount_path,
            None::<&str>,
            mount_flags,
            None::<&str>,
        )?;

        Ok(())
    }

    fn chroot(&self) -> Result<(), Box<dyn std::error::Error>> {
        info!("Chrooting to {:?}", self.mount_dir);

        // Save working directory
        let current_dir: PathBuf = env::current_dir()?;
        // Chroot
        chroot(&self.mount_dir)?;
        // Switch back to working directory
        env::set_current_dir(&current_dir)?;

        Ok(())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let current_exe = env::current_exe()?;
    let current_dir = current_exe.parent().unwrap();
    info!("Current directory: {:?}", current_dir);

    let nix_dir: PathBuf = current_dir.join("nix");
    if !nix_dir.exists() {
        error!("nix directory does not exist");
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "nix directory does not exist",
        )));
    }

    let mount_dir = current_dir.join("mountroot");
    if !mount_dir.exists() {
        error!("mount directory does not exist");
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "mount directory does not exist",
        )));
    }

    let entrypoint = current_dir.join("entrypoint");
    let entrypoint_link = fs::symlink_metadata(&entrypoint);
    if let Err(e) = entrypoint_link {
        error!("entrypoint does not exist or is not a symbolic link");
        return Err(Box::new(e));
    }

    let app = AppRun {
        mount_dir,
        nix_dir,
        debug: false,
        entrypoint,
    };
    app.exec()?;

    Ok(())
}
