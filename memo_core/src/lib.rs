mod btree;
mod buffer;
mod epoch;
#[allow(non_snake_case, unused_imports)]
mod operation_queue;
mod serialization;
pub mod time;
mod work_tree;

pub use crate::buffer::{Buffer, Change, Point};
pub use crate::epoch::{Cursor, DirEntry, Epoch, FileStatus, FileType, ROOT_FILE_ID};
pub use crate::work_tree::{
    BufferId, BufferSelectionRanges, ChangeObserver, GitProvider, LocalSelectionSetId, Operation,
    OperationEnvelope, WorkTree,
};
use std::borrow::Cow;
use std::fmt;
use std::io;
use uuid::Uuid;

pub type ReplicaId = Uuid;
pub type Oid = [u8; 20];

#[derive(Debug)]
pub enum Error {
    IoError(io::Error),
    DeserializeError,
    InvalidPath(Cow<'static, str>),
    InvalidOperations,
    InvalidFileId(Cow<'static, str>),
    InvalidBufferId,
    InvalidDirEntry,
    InvalidOperation,
    InvalidSelectionSet(buffer::SelectionSetId),
    InvalidLocalSelectionSet(LocalSelectionSetId),
    InvalidAnchor(Cow<'static, str>),
    OffsetOutOfRange,
    CursorExhausted,
}

trait ReplicaIdExt {
    fn to_flatbuf(&self) -> serialization::ReplicaId;
    fn from_flatbuf(message: &serialization::ReplicaId) -> Self;
}

impl ReplicaIdExt for ReplicaId {
    fn to_flatbuf(&self) -> serialization::ReplicaId {
        fn u64_from_bytes(bytes: &[u8]) -> u64 {
            let mut n = 0;
            for i in 0..8 {
                n |= (bytes[i] as u64) << i * 8;
            }
            n
        }

        let bytes = self.as_bytes();
        serialization::ReplicaId::new(u64_from_bytes(&bytes[0..8]), u64_from_bytes(&bytes[8..16]))
    }

    fn from_flatbuf(message: &serialization::ReplicaId) -> Self {
        fn bytes_from_u64(n: u64) -> [u8; 8] {
            let mut bytes = [0; 8];
            for i in 0..8 {
                bytes[i] = (n >> i * 8) as u8;
            }
            bytes
        }

        let mut bytes = [0; 16];
        bytes[0..8].copy_from_slice(&bytes_from_u64(message.first_8_bytes()));
        bytes[8..16].copy_from_slice(&bytes_from_u64(message.last_8_bytes()));

        Uuid::from_bytes(bytes)
    }
}

impl From<Error> for String {
    fn from(error: Error) -> Self {
        format!("{:?}", error)
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Error::IoError(error)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

impl PartialEq for Error {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Error::IoError(err_1), Error::IoError(err_2)) => {
                err_1.kind() == err_2.kind() && err_1.to_string() == err_2.to_string()
            }
            (Error::DeserializeError, Error::DeserializeError) => true,
            (Error::InvalidPath(err_1), Error::InvalidPath(err_2)) => err_1 == err_2,
            (Error::InvalidOperations, Error::InvalidOperations) => true,
            (Error::InvalidFileId(err_1), Error::InvalidFileId(err_2)) => err_1 == err_2,
            (Error::InvalidBufferId, Error::InvalidBufferId) => true,
            (Error::InvalidDirEntry, Error::InvalidDirEntry) => true,
            (Error::InvalidOperation, Error::InvalidOperation) => true,
            (Error::InvalidSelectionSet(id_1), Error::InvalidSelectionSet(id_2)) => id_1 == id_2,
            (Error::InvalidLocalSelectionSet(id_1), Error::InvalidLocalSelectionSet(id_2)) => {
                id_1 == id_2
            }
            (Error::InvalidAnchor(err_1), Error::InvalidAnchor(err_2)) => err_1 == err_2,
            (Error::OffsetOutOfRange, Error::OffsetOutOfRange) => true,
            (Error::CursorExhausted, Error::CursorExhausted) => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::ReplicaId;
    use rand::Rng;
    use std::collections::BTreeMap;

    #[derive(Clone)]
    struct Envelope<T: Clone> {
        message: T,
        sender: ReplicaId,
    }

    pub(crate) struct Network<T: Clone> {
        inboxes: BTreeMap<ReplicaId, Vec<Envelope<T>>>,
        all_messages: Vec<T>,
    }

    impl<T: Clone> Network<T> {
        pub fn new() -> Self {
            Network {
                inboxes: BTreeMap::new(),
                all_messages: Vec::new(),
            }
        }

        pub fn add_peer(&mut self, id: ReplicaId) {
            self.inboxes.insert(id, Vec::new());
        }

        pub fn is_idle(&self) -> bool {
            self.inboxes.values().all(|i| i.is_empty())
        }

        pub fn all_messages(&self) -> &Vec<T> {
            &self.all_messages
        }

        pub fn broadcast<R>(&mut self, sender: ReplicaId, messages: Vec<T>, rng: &mut R)
        where
            R: Rng,
        {
            for (replica, inbox) in self.inboxes.iter_mut() {
                if *replica != sender {
                    for message in &messages {
                        let min_index = inbox
                            .iter()
                            .enumerate()
                            .rev()
                            .find_map(|(index, envelope)| {
                                if sender == envelope.sender {
                                    Some(index + 1)
                                } else {
                                    None
                                }
                            })
                            .unwrap_or(0);

                        // Insert one or more duplicates of this message *after* the previous
                        // message delivered by this replica.
                        for _ in 0..rng.gen_range(1, 4) {
                            let insertion_index = rng.gen_range(min_index, inbox.len() + 1);
                            inbox.insert(
                                insertion_index,
                                Envelope {
                                    message: message.clone(),
                                    sender,
                                },
                            );
                        }
                    }
                }
            }
            self.all_messages.extend(messages);
        }

        pub fn has_unreceived(&self, receiver: ReplicaId) -> bool {
            !self.inboxes[&receiver].is_empty()
        }

        pub fn receive<R>(&mut self, receiver: ReplicaId, rng: &mut R) -> Vec<T>
        where
            R: Rng,
        {
            let inbox = self.inboxes.get_mut(&receiver).unwrap();
            let count = rng.gen_range(0, inbox.len() + 1);
            inbox
                .drain(0..count)
                .map(|envelope| envelope.message)
                .collect()
        }

        pub fn clear_unreceived(&mut self, receiver: ReplicaId) {
            self.inboxes.get_mut(&receiver).unwrap().clear();
        }
    }
}
