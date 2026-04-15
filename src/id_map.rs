use nix::unistd::{Gid, Uid};
use paste::paste;
use std::fmt;

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

        impl fmt::Display for $struct {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(
                    f,
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
