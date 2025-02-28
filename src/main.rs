use std::{
    env,
    ffi::CString,
    fs,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::Duration,
};

use clap::Parser;
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

#[derive(Parser, Debug)]
#[command(author, about)]
struct Cli {
    #[arg(long)]
    bind: Option<Vec<PathBuf>>,
    #[arg(long)]
    nix_dir: Option<PathBuf>,
    #[arg(long)]
    entrypoint: Option<PathBuf>,
    #[arg(long)]
    mount_dir: Option<PathBuf>,
    #[arg(long)]
    version: bool,
    #[arg(long, default_value_t = 5.0)]
    mount_timeout: f32,
}

#[derive(Debug, Default)]
struct AppRun {
    binds: Option<Vec<PathBuf>>,
    nix_dir: PathBuf,
    mount_dir: PathBuf,
    entrypoint: PathBuf,
    args: Vec<String>,
    new_user_namespace: bool,
    mount_timeout: f32,
}

/// Test if a file is openable
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
    /// Execute the entrypoint
    fn exec_in_chroot(mut self) -> Result<(), Box<dyn std::error::Error>> {
        if !Uid::effective().is_root() {
            self.new_user_namespace = true;
        }
        self.mounts()?;
        self.chroot()?;

        // Execute a shell
        // https://stackoverflow.com/questions/38948669/whats-the-most-direct-way-to-convert-a-path-to-a-c-char
        let cmd = CString::new(self.entrypoint.as_os_str().to_str().unwrap())?;
        let args: Vec<CString> = self
            .args
            .into_iter()
            .map(|s| CString::new(s).unwrap())
            .collect();
        info!("Executing entrypoint with {:?}", args);
        execve(&cmd, &args, &[CString::new("TERM=xterm-256color")?])?;

        Ok(())
    }

    /// Write uid_map and gid_map
    fn write_id_maps(&self, uid: Uid, gid: Gid) -> Result<(), std::io::Error> {
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

    /// Find if file exists in a given time
    fn with_timeout<F, T>(&self, f: F) -> Result<T, mpsc::RecvTimeoutError>
    where
        F: FnOnce() -> T,
        F: Send + 'static,
        T: Send + 'static,
    {
        let (sender, receiver) = mpsc::channel();
        let _t = thread::spawn(move || {
            sender.send(f()).unwrap_or(());
        });

        receiver.recv_timeout(Duration::from_secs_f32(self.mount_timeout))
    }

    /// Perform a recursive bind mount
    fn rec_bind_mount(&self, path: &PathBuf, mount_path: &PathBuf) -> Result<(), std::io::Error> {
        // https://www.kernel.org/doc/Documentation/filesystems/sharedsubtree.txt
        let mount_flags = {
            // Recursively bind mount
            MsFlags::MS_BIND | MsFlags::MS_REC |
            // Make this mount point a slave so that mounts in the container don't propagate to the host
            MsFlags::MS_SLAVE |
            MsFlags::MS_UNBINDABLE
        };
        let path_name = path.file_name().unwrap();

        let mount_result = if path.is_dir() {
            // Create bind mount
            debug!("Creating bind mount for {path_name:?}");
            fs::create_dir_all(mount_path)?;
            mount::<_, _, Path, Path>(Some(path), mount_path, None, mount_flags, None)
        } else {
            // Create a file and bind mount it
            debug!("Creating bind mount for {path_name:?}");
            fs::write(mount_path, "")?;
            mount::<_, _, Path, Path>(Some(path), mount_path, None, mount_flags, None)
        };

        if let Err(e) = mount_result {
            warn!("Failed to mount {path_name:?}: {e:?}");
        }

        Ok(())
    }

    /// Mount all nonexist subdirectories of /nix/store from host
    fn mount_nix(&self, host_nix: &Path, mount_nix: &Path) -> Result<(), std::io::Error> {
        let host_store = host_nix.join("store");
        let mount_store = mount_nix.join("store");
        if !host_store.exists() {
            return Ok(());
        }
        if !mount_store.exists() {
            fs::create_dir_all(&mount_store)?;
        }

        info!("Mounting {host_store:?}/* to {mount_store:?}");
        for entry in host_store.read_dir()? {
            let path = entry?.path();
            if !path.is_dir() {
                continue;
            }

            // Check if this directory exists in the container
            let mount_path = mount_store.join(path.file_name().unwrap());
            if mount_path.exists() {
                continue;
            }

            // Create a bind mount
            self.rec_bind_mount(&path, &mount_path)?;
        }

        Ok(())
    }

    /// Create a new mount namespace, bind mount everything from / into the mount_dir,
    /// and bind mount /nix from self.nix_to_mount
    fn mounts(&self) -> Result<(), std::io::Error> {
        let (uid, gid) = (Uid::current(), Gid::current());
        debug!("Current uid: {uid}, gid: {gid}");

        // Create a new mount namespace
        let clone_flags = if self.new_user_namespace {
            CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNS
        } else {
            CloneFlags::CLONE_NEWNS
        };
        info!("Creating new mount namespace with {clone_flags:?}");
        if let Err(e) = unshare(clone_flags) {
            if !self.new_user_namespace {
                error!("Failed to create new mount namespace: {e:?}. Did you forget to run me as root?");
            } else {
                error!("Failed to create new mount namespace: {e:?}.");
            }
        }

        if clone_flags.contains(CloneFlags::CLONE_NEWUSER) {
            info!("Created new user namespace");
            self.write_id_maps(uid, gid)?;
        }

        // Mark all mount points as slave
        // So that mounts in the container don't propagate to the host
        // For example, when we unmount /nix in the container, we don't want that to propagate to the host
        info!("Mounting / as rslave");
        mount(
            None::<&str>,
            "/",
            None::<&str>,
            MsFlags::MS_SLAVE | MsFlags::MS_REC,
            None::<&str>,
        )?;

        // Mount a tmpfs
        info!("Mounting tmpfs to {:?}", self.mount_dir);
        mount(
            Some("tmpfs"),
            &self.mount_dir,
            Some("tmpfs"),
            MsFlags::MS_NOSUID,
            Some("mode=755"),
        )?;

        let mut paths_to_bind = vec![];
        if let Some(binds) = self.binds.as_ref() {
            // Bind mount everything from / into the mount_dir
            for bind in binds {
                let path = PathBuf::from(bind);
                paths_to_bind.push(path);
            }
        } else {
            // Copy over root directories
            let files = fs::read_dir("/")?;
            for file in files {
                let path = file?.path();
                paths_to_bind.push(path);
            }
        }

        for path in paths_to_bind {
            let path_name = path.file_name().unwrap();
            let mount_path = self.mount_dir.join(path_name);

            if path_name == "nix" {
                continue;
            }

            let check_path = path.clone();
            let exists = match self.with_timeout(move || check_path.try_exists()) {
                Err(e) => {
                    warn!("Error: {}", e.to_string());
                    warn!("Timed out to check existance of {path_name:?}. Maybe it's a broken symlink or broken NFS mount?");
                    false
                }
                Ok(Err(e)) => {
                    warn!("Error: {}", e.to_string());
                    warn!("Failed to check existance of {path_name:?}.");
                    false
                }
                Ok(Ok(exists)) => exists,
            };

            if !exists {
                warn!("Skipping non-existent or error path {:?}", path);
                continue;
            }

            self.rec_bind_mount(&path, &mount_path)?;
        }

        // Bind mount /nix from self.nix_to_mount
        let mount_path = self.mount_dir.join("nix");
        fs::create_dir_all(&mount_path)?;
        info!("Creating bind mount for /nix from {:?}", self.nix_dir);
        self.rec_bind_mount(&self.nix_dir, &mount_path)?;

        Ok(())
    }

    /// Chroot to self.mount_dir
    fn chroot(&self) -> Result<(), Box<dyn std::error::Error>> {
        info!("Chrooting to {:?}", self.mount_dir);

        // Save working directory
        let current_dir: PathBuf = env::current_dir()?;
        // Chroot
        chroot(&self.mount_dir)?;
        // Switch back to working directory
        env::set_current_dir(current_dir)?;

        Ok(())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Oply keep --apprun-xxx flags and replace that with --xxx

    let mut args = std::env::args();
    let arg0 = args.next().unwrap_or_else(|| "nix-apprun".to_string());

    let mut apprun_args = vec![arg0.clone()];
    let mut pass_args = vec![arg0];
    for arg in args {
        if arg.starts_with("--apprun-") {
            apprun_args.push(arg.replace("--apprun-", "--"));
        } else {
            pass_args.push(arg);
        }
    }

    // let cli = Cli::parse();
    let cli = Cli::parse_from(apprun_args);

    if cli.version {
        println!("nix-apprun v{}", env!("CARGO_PKG_VERSION"));
    }

    env_logger::init();

    let current_exe = env::current_exe()?;
    let current_dir = current_exe.parent().unwrap();
    info!("Current directory: {:?}", current_dir);

    let nix_dir = if let Some(nix_dir) = cli.nix_dir {
        nix_dir
    } else {
        current_dir.join("nix")
    };
    if !nix_dir.exists() {
        error!("nix directory does not exist");
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "nix directory does not exist",
        )));
    }

    let mount_dir = if let Some(mount_dir) = cli.mount_dir {
        mount_dir
    } else {
        current_dir.join("mountroot")
    };
    if !mount_dir.exists() {
        error!("mount directory does not exist");
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "mount directory does not exist",
        )));
    }

    let entrypoint = if let Some(entrypoint) = cli.entrypoint {
        entrypoint
    } else {
        let entrypoint = current_dir.join("entrypoint");
        let entrypoint_link = fs::symlink_metadata(&entrypoint);
        if let Err(e) = entrypoint_link {
            error!("entrypoint does not exist or is not a symbolic link");
            return Err(Box::new(e));
        }
        entrypoint
    };

    let app = AppRun {
        mount_dir,
        nix_dir,
        entrypoint,
        args: pass_args,
        binds: cli.bind,
        mount_timeout: cli.mount_timeout,
        ..Default::default()
    };
    app.exec_in_chroot()?;

    Ok(())
}
