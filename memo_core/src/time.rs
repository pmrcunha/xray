use crate::serialization;
use crate::Error;
use crate::ReplicaId;
use crate::ReplicaIdExt;
use flatbuffers::{FlatBufferBuilder, WIPOffset};
use serde::{Deserializer, Serializer};
use serde_derive::{Deserialize, Serialize};
use std::cmp::{self, Ordering};
use std::collections::HashMap;
use std::mem;
use std::ops::{Add, AddAssign};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct Local {
    pub replica_id: ReplicaId,
    pub value: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Global(
    #[serde(
        serialize_with = "Global::serialize_inner",
        deserialize_with = "Global::deserialize_inner"
    )]
    Arc<HashMap<ReplicaId, u64>>,
);

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
pub struct Lamport {
    pub value: u64,
    pub replica_id: ReplicaId,
}

impl Local {
    pub fn new(replica_id: ReplicaId) -> Self {
        Self {
            replica_id,
            value: 1,
        }
    }

    pub fn tick(&mut self) -> Self {
        let timestamp = *self;
        self.value += 1;
        timestamp
    }

    pub fn observe(&mut self, timestamp: Self) {
        if timestamp.replica_id == self.replica_id {
            self.value = cmp::max(self.value, timestamp.value + 1);
        }
    }

    pub fn to_flatbuf(&self) -> serialization::Timestamp {
        serialization::Timestamp::new(self.value, &self.replica_id.to_flatbuf())
    }

    pub fn from_flatbuf(message: &serialization::Timestamp) -> Self {
        Self {
            value: message.value(),
            replica_id: ReplicaId::from_flatbuf(message.replica_id()),
        }
    }
}

impl<'a> Add<&'a Self> for Local {
    type Output = Local;

    fn add(self, other: &'a Self) -> Self::Output {
        cmp::max(&self, other).clone()
    }
}

impl<'a> AddAssign<&'a Local> for Local {
    fn add_assign(&mut self, other: &Self) {
        if *self < *other {
            *self = other.clone();
        }
    }
}

impl Global {
    pub fn new() -> Self {
        Global(Arc::new(HashMap::new()))
    }

    fn serialize_inner<S>(
        inner: &Arc<HashMap<ReplicaId, u64>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::Serialize;
        inner.serialize(serializer)
    }

    fn deserialize_inner<'de, D>(deserializer: D) -> Result<Arc<HashMap<ReplicaId, u64>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::Deserialize;
        Ok(Arc::new(HashMap::deserialize(deserializer)?))
    }

    pub fn get(&self, replica_id: ReplicaId) -> u64 {
        *self.0.get(&replica_id).unwrap_or(&0)
    }

    pub fn observe(&mut self, timestamp: Local) {
        let map = Arc::make_mut(&mut self.0);
        let value = map.entry(timestamp.replica_id).or_insert(0);
        *value = cmp::max(*value, timestamp.value);
    }

    pub fn observe_all(&mut self, other: &Self) {
        for (replica_id, value) in other.0.as_ref() {
            self.observe(Local {
                replica_id: *replica_id,
                value: *value,
            });
        }
    }

    pub fn observed(&self, timestamp: Local) -> bool {
        self.get(timestamp.replica_id) >= timestamp.value
    }

    pub fn changed_since(&self, other: &Self) -> bool {
        self.0
            .iter()
            .any(|(replica_id, value)| *value > other.get(*replica_id))
    }

    pub fn to_flatbuf<'fbb>(
        &self,
        builder: &mut FlatBufferBuilder<'fbb>,
    ) -> WIPOffset<serialization::GlobalTimestamp<'fbb>> {
        builder.start_vector::<serialization::Timestamp>(self.0.len());
        for (replica_id, value) in self.0.as_ref() {
            builder.push(&serialization::Timestamp::new(
                *value,
                &replica_id.to_flatbuf(),
            ));
        }
        let timestamps = Some(builder.end_vector(self.0.len()));
        serialization::GlobalTimestamp::create(
            builder,
            &serialization::GlobalTimestampArgs { timestamps },
        )
    }

    pub fn from_flatbuf<'fbb>(
        message: serialization::GlobalTimestamp<'fbb>,
    ) -> Result<Self, Error> {
        let mut local_timestamps = HashMap::new();
        for local_timestamp in message.timestamps().ok_or(Error::DeserializeError)? {
            let replica_id = ReplicaId::from_flatbuf(local_timestamp.replica_id());
            let value = local_timestamp.value();
            local_timestamps.insert(replica_id, value);
        }
        Ok(Global(Arc::new(local_timestamps)))
    }
}

impl PartialOrd for Global {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        let mut global_ordering = Ordering::Equal;

        for replica_id in self.0.keys().chain(other.0.keys()) {
            let ordering = self.get(*replica_id).cmp(&other.get(*replica_id));
            if ordering != Ordering::Equal {
                if global_ordering == Ordering::Equal {
                    global_ordering = ordering;
                } else if ordering != global_ordering {
                    return None;
                }
            }
        }

        Some(global_ordering)
    }
}

impl Lamport {
    pub fn new(replica_id: ReplicaId) -> Self {
        Self {
            value: 1,
            replica_id,
        }
    }

    pub fn tick(&mut self) -> Self {
        let timestamp = *self;
        self.value += 1;
        timestamp
    }

    pub fn observe(&mut self, timestamp: Self) {
        self.value = cmp::max(self.value, timestamp.value) + 1;
    }

    pub fn to_flatbuf(&self) -> serialization::Timestamp {
        serialization::Timestamp::new(self.value, &self.replica_id.to_flatbuf())
    }

    pub fn from_flatbuf(message: &serialization::Timestamp) -> Self {
        Self {
            value: message.value(),
            replica_id: ReplicaId::from_flatbuf(message.replica_id()),
        }
    }

    pub fn to_bytes(&self) -> [u8; 24] {
        let mut bytes = [0; 24];
        bytes[0..8].copy_from_slice(unsafe { &mem::transmute::<u64, [u8; 8]>(self.value.to_be()) });
        bytes[8..24].copy_from_slice(self.replica_id.as_bytes());
        bytes
    }
}
