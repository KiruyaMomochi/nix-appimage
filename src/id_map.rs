use nix::unistd::{Gid, Uid};
use paste::paste;
use std::{fmt::Debug, path::PathBuf, str::FromStr};

macro_rules! id_map {
    ($id:ident) => {
        paste! { id_map!($id, [<$id Map>]); }
    };
    ($id:ident, $struct:ident) => {
        #[derive(Debug)]
        pub struct $struct {
            pub inside_id: $id,
            pub outside_id: $id,
            pub count: u32,
        }

        impl FromStr for $struct {
            type Err = std::num::ParseIntError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let mut parts = s.split_whitespace();
                let inside_id = parts.next().unwrap().parse::<nix::libc::uid_t>()?;
                let outside_id = parts.next().unwrap().parse::<nix::libc::uid_t>()?;
                let count = parts.next().unwrap().parse::<nix::libc::uid_t>()?;
                Ok($struct {
                    inside_id: $id::from_raw(inside_id),
                    outside_id: $id::from_raw(outside_id),
                    count,
                })
            }
        }

        impl ToString for $struct {
            fn to_string(&self) -> String {
                format!(
                    "{inside_id} {outside_id} {count}",
                    inside_id = self.inside_id,
                    outside_id = self.outside_id,
                    count = self.count
                )
            }
        }
    };
}

id_map!(Uid);
id_map!(Gid);

pub fn read_uid_map() -> Result<Vec<UidMap>, std::io::Error> {
    let uid_map_file = PathBuf::from("/proc/self/uid_map");
    let uidmap = std::fs::read_to_string(uid_map_file)?
        .lines()
        .map(|x| UidMap::from_str(x).unwrap())
        .collect();
    Ok(uidmap)
}
