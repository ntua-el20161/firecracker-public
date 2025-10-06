use std::sync::atomic::AtomicU32;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::*;
use crate::devices::virtio::persist::VirtioDeviceState;
use crate::vstate::memory::GuestMemoryMmap;
use crate::snapshot::Persist;
use crate::devices::virtio::queue::FIRECRACKER_MAX_QUEUE_SIZE;
use crate::devices::virtio::TYPE_MEMORY;
use crate::devices::virtio::device::DeviceState;
use crate::devices::virtio::memory::device::{ConfigSpace, MemBitmap};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VirtioMemConfigSpaceState {
    block_size: u64,
    node_id: u16,
    addr: u64, 
    region_size: u64,
    usable_region_size: u64,
    plugged_size: u64,
    requested_size: u64
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtioMemState {
    virtio_state: VirtioDeviceState,
    config_space: VirtioMemConfigSpaceState,
    id: String,
    memory_bitmap: MemBitmap,
    host_addr: u64,
}

#[derive(Debug)]
pub struct VirtioMemConstructorArgs {
    pub mem: GuestMemoryMmap,
}

impl Persist<'_> for Memory {
    type State = VirtioMemState;
    type ConstructorArgs = VirtioMemConstructorArgs;
    type Error = super::MemoryDeviceError;

    fn save(&self) -> Self::State {
        VirtioMemState {
            virtio_state: VirtioDeviceState::from_device(self),
            config_space: VirtioMemConfigSpaceState {
                block_size: self.block_size(),
                node_id: self.node_id(),
                addr: self.config_space.addr,
                region_size: self.config_space.region_size,
                usable_region_size: self.config_space.usable_region_size,
                plugged_size: self.config_space.plugged_size,
                requested_size: self.config_space.requested_size
            },
            id: self.id.clone(),
            host_addr: self.host_addr,
            memory_bitmap: self.memory_bitmap.clone()
        }
    }

    fn restore(
        guest_memory: Self::ConstructorArgs,
        state: &Self::State,
    ) -> Result<Self, Self::Error> {
        let mut virtio_mem = Memory::new(
            state.config_space.block_size,
            Some(state.config_space.node_id),
            state.config_space.region_size,
            state.id.clone(),
            state.config_space.requested_size
        )?;
        virtio_mem.queues = state
            .virtio_state
            .build_queues_checked(
                &guest_memory.mem,
                TYPE_MEMORY,
                1,
                FIRECRACKER_MAX_QUEUE_SIZE
            )
            .map_err(|_| Self::Error::QueueRestoreError)?;
        virtio_mem.irq_trigger.irq_status = 
            Arc::new(AtomicU32::new(state.virtio_state.interrupt_status));
        virtio_mem.avail_features = state.virtio_state.avail_features;
        virtio_mem.acked_features = state.virtio_state.acked_features;
        virtio_mem.config_space.addr = state.config_space.addr;
        virtio_mem.config_space.plugged_size = state.config_space.plugged_size;
        virtio_mem.addr_is_set = true;
        virtio_mem.host_addr_is_set = true;
        virtio_mem.host_addr = state.host_addr;
        virtio_mem.memory_bitmap = state.memory_bitmap.clone();

        if state.virtio_state.activated {
            virtio_mem.device_state = DeviceState::Activated(guest_memory.mem);
        }

        Ok(virtio_mem)
    }
}

