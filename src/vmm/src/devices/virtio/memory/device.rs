// Copyright 2022 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use std::cmp;
use std::io::Write;
use std::result::Result;
use std::mem::size_of;
use crate::devices::virtio::memory::GUEST_REQUESTS_INDEX;
use crate::logger::{error, info, debug};
use micro_http::Response;
use utils::eventfd::EventFd;
use utils::get_page_size;
use vm_memory::{Bytes };
use crate::devices::virtio::gen::virtio_blk::VIRTIO_F_VERSION_1;
use serde::{Serialize, Deserialize};

//use vm_memory::{ByteValued, GuestMemoryMmap};
use crate::vstate::memory::{ByteValued, GuestMemoryMmap};
use super::{
    VIRTIO_MEM_REQ_PLUG,
    VIRTIO_MEM_REQ_UNPLUG,
    VIRTIO_MEM_REQ_UNPLUG_ALL,
    VIRTIO_MEM_REQ_STATE,
    VIRTIO_MEM_RESP_ACK,
    VIRTIO_MEM_RESP_NACK,
    VIRTIO_MEM_RESP_BUSY,
    VIRTIO_MEM_RESP_ERROR,
    VIRTIO_MEM_STATE_PLUGGED,
    VIRTIO_MEM_STATE_UNPLUGGED,
    VIRTIO_MEM_STATE_MIXED,
};
use super::super::{ActivateError, TYPE_MEMORY};
use crate::devices::virtio::device::DeviceState;
use crate::devices::virtio::device::VirtioDevice;
use crate::devices::virtio::queue::Queue;
use super::{MemoryResult, QUEUE_SIZE};
use crate::devices::virtio::memory::MemoryDeviceError as MemoryError;
use crate::devices::virtio::device::{ IrqTrigger, IrqType };

const KIB: u64 = 1024;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ConfigSpace {
    pub block_size: u64,
    pub node_id: u16,
    _padding: [u8; 6],

    // guest physical addres from where the memory region starts
    pub addr: u64, 
    pub region_size: u64,
    pub usable_region_size: u64,
    pub plugged_size: u64,
    pub requested_size: u64
}
// Safe because ConfigSpace only contains plain data.
unsafe impl ByteValued for ConfigSpace {}

// Configuration for the memory device.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryConfig {
    /// ID of the device.
    pub id: String,
    /// Block size in bytes.
    #[serde(default)]
    pub block_size_kib: u64,
    /// Node id if any.
    #[serde(default)]
    pub node_id: Option<u16>,
    /// Region size in bytes.
    pub region_size_kib: u64,
    /// Requested size in bytes.
    #[serde(default)]
    pub requested_size_kib: u64,
}

/// Virtio memory device.
#[derive(Debug)]
pub struct Memory {
    // Virtio fields.
    pub(crate) avail_features: u64,
    pub(crate) acked_features: u64,
    pub(crate) config_space: ConfigSpace,
    pub(crate) activate_evt: EventFd,
    // Transport related fields.
    pub(crate) queues: Vec<Queue>,  // virtq
    pub(crate) queue_evts: [EventFd; 1], // used for notification
    pub(crate) device_state: DeviceState, // Device state : Activated/Inactive
    pub(crate) irq_trigger: IrqTrigger, 
    // Implementation specific fields.
    pub(crate) id: String,
    pub(crate) addr_is_set: bool,
    pub(crate) memory_bitmap: MemBitmap,
    pub(crate) host_addr: u64,
    pub(crate) host_addr_is_set: bool,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VirtioMemReqData {
    pub addr: u64,
    pub nb_blocks: u16,
    pub padding: [u16; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VirtioMemReq {
    pub req_type: u16, 
    pub padding: [u16; 3],
    pub data: VirtioMemReqData // the union field
}

unsafe impl ByteValued for VirtioMemReq {}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VirtioMemResp {
    pub resp_type: u16,
    pub padding: [u16; 3],
    pub resp_state: u16,
}

unsafe impl ByteValued for VirtioMemResp {}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MemBitmap {
    bitmap: Vec<bool>,
}

impl MemBitmap {
    pub fn new(region_size: u64, block_size: u64) -> Self {
        MemBitmap {
            bitmap: vec![false; (region_size / block_size) as usize],
        }
    }

    fn is_range_state(&self, first_block_index: usize, nb_blocks: u16, plugged: bool) -> bool {
        for block in self
            .bitmap
            .iter()
            .skip(first_block_index)
            .take(nb_blocks as usize) 
        {
            if *block != plugged {
                return false;
            }
        }
        true
    }

    fn set_range_state(&mut self, first_block_index: usize, nb_blocks: u16, plugged: bool) {
        for block in self
            .bitmap.iter_mut()
            .skip(first_block_index)
            .take(nb_blocks as usize) 
        {
            *block = plugged;
        }
    }
}

impl VirtioMemResp {
    pub fn new(resp_type: u16, resp_state: u16) -> Self {
        Self {
            resp_type,
            padding: Default::default(),
            resp_state,
        }
    }
}

impl Memory {
    pub fn new(
        block_size: u64,
        node_id: Option<u16>,
        region_size: u64,
        id: String,
        requested_size: u64,
    ) -> MemoryResult<Memory> {
        // the way thsi device will handle hot(un)plugs requires the block size
        // to be a multiple of the host page size
        //        
        // memory device is only available for 64 bit platforms
        let page_size: u64 = get_page_size().map_err(MemoryError::PageSize)? as u64;
        if block_size == 0 {
            return Err(MemoryError::BlockSizeIsZero);
        }
        if block_size % page_size != 0 {
            return Err(MemoryError::BlockSizeNotMultipleOfPageSize(page_size));
        }
        // virtio-mem spec requirement
        if !block_size.is_power_of_two() {
            return Err(MemoryError::BlockSizeNotPowerOf2);
        }
        if region_size % block_size != 0 {
            return Err(MemoryError::SizeNotMultipleOfBlockSize);
        }
        if requested_size % block_size != 0 {
            return Err(MemoryError::SizeNotMultipleOfBlockSize);
        }
        /*if let Some(node_id) = node_id {
            todo!("Node id feature unimplemented [{}]", node_id);
        }*/
        let avail_features = 1u64 << VIRTIO_F_VERSION_1;
        let queue_evts = [EventFd::new(libc::EFD_NONBLOCK).map_err(MemoryError::EventFd)?];
        info!(
            "Memory::new({}, {:?}, {}, {})",
            block_size, node_id, region_size, id
        );
        let bitmap = MemBitmap::new(region_size, block_size);
        Ok(Memory {
            avail_features,
            acked_features: 0u64,
            addr_is_set: false,
            memory_bitmap: bitmap,
            config_space: ConfigSpace {
                block_size,
                node_id: node_id.unwrap_or_default(),
                _padding: [0u8; 6],
                addr: 0u64,
                region_size,
                usable_region_size: region_size,
                plugged_size: 0u64,
                requested_size: requested_size,
            },
            id,
            irq_trigger: IrqTrigger::new().map_err(MemoryError::EventFd)?,
            device_state: DeviceState::Inactive,
            activate_evt: EventFd::new(libc::EFD_NONBLOCK).map_err(MemoryError::EventFd)?,
            queues: vec![Queue::new(QUEUE_SIZE)],
            queue_evts,
            host_addr: 0u64, // host_addr and addr are set after the creation of the memory region
            host_addr_is_set: false,
        })
    }
 
    /// Process device virtio queue.
    pub(crate) fn process_guest_request_queue(&mut self) {
        // called in src>vmm>src>device_manager>mmio.rs>kick_devices
        // called in case the device has memory to plug on init (?) virtio spec-1.2 5.15.
        info!("Memory.process_guest_requests_queue");
        let _ = self.process_request_queue_event();
    }

    fn is_request_range_valid(&self, addr: u64, size: u64) -> bool {
        let usable_region_end = self.config_space.addr + self.config_space.usable_region_size;
        if  addr % self.block_size() != 0
            || size == 0
            || addr < self.config_space.addr
            || addr.checked_add(size).is_some_and(|end| end > usable_region_end)
        {
            return false;
        }
        true
    }

    pub(crate) fn process_plug(&mut self, addr: u64, nb_blocks: u16) -> (u16, u16) {
        info!("Memory.process_plug");
        let plug_size = nb_blocks as u64 * self.block_size();
        
        if !self.is_request_range_valid(addr, plug_size) {
            info!("Plug request invalid: addr {}, nb_blocks {}", addr, nb_blocks);
            return (VIRTIO_MEM_RESP_ERROR, 0)
        }
        
        let offset = addr - self.config_space.addr;
        let first_block_index = (offset / self.block_size()) as usize;

        // check if any of the request blocks is in plugged state in the bitmap
        if self.memory_bitmap.is_range_state(first_block_index, nb_blocks, false) {
            info!("Plug request overlaps with already plugged blocks: addr {}, nb_blocks {}", addr, nb_blocks);
            return (VIRTIO_MEM_RESP_ERROR, 0);
        }

        self.memory_bitmap.set_range_state(first_block_index, nb_blocks, true);

        self.config_space.plugged_size += plug_size;

        info!("succesful plug of {} blocks at addr {}", nb_blocks, addr);
        (VIRTIO_MEM_RESP_ACK, 0)
    }

    pub(crate) fn process_unplug(&mut self, addr: u64, nb_blocks: u16) -> (u16, u16) {
        //info!("Memory.process_unplug");
        
        let unplug_size = nb_blocks as u64 * self.block_size();
        if !self.is_request_range_valid(addr, unplug_size) {
            info!("Unplug request invalid: addr {}, nb_blocks {}", addr, nb_blocks);
            return (VIRTIO_MEM_RESP_ERROR, 0);
        }
        
        let offset = addr - self.config_space.addr;
        let first_block_index = (offset / self.block_size()) as usize;

        if self.memory_bitmap.is_range_state(first_block_index, nb_blocks, true) {
            info!("Unplug request overlaps with already unplugged blocks: addr {}, nb_blocks {}", addr, nb_blocks);
            return (VIRTIO_MEM_RESP_ERROR, 0);
        }

        let res = unsafe {
            libc::madvise(
                (self.host_addr + offset) as *mut libc::c_void,
                unplug_size as libc::size_t,
                libc::MADV_DONTNEED
            )
        };

        if res != 0 {
            error!("Failed to madvise memory region: addr {}, size {}", addr, unplug_size);
            return (VIRTIO_MEM_RESP_ERROR, 0);
        }

        self.memory_bitmap.set_range_state(first_block_index, nb_blocks, false);

        self.config_space.plugged_size -= unplug_size;

        //info!("succesful unplug of {} blocks at addr {}", nb_blocks, addr);
        (VIRTIO_MEM_RESP_ACK, 0)
    }
    pub(crate) fn process_unplug_all(&mut self) -> (u16, u16) {

        let region_size = self.region_size();
        let block_size = self.block_size();
        let res = unsafe {
            libc::madvise(
                (self.host_addr) as *mut libc::c_void,
                region_size as libc::size_t,    
                libc::MADV_DONTNEED
            )
        };
        if res != 0 {
            error!("Failed to madvise memory region: addr {}, size {}", self.config_space.addr, region_size);
            return (VIRTIO_MEM_RESP_ERROR, 0)
        }

        let bitmap = &mut self.memory_bitmap;
        bitmap.set_range_state(0, (region_size / block_size) as u16, false);

        self.config_space.plugged_size = 0;

        info!("Successful unplug of all blocks");   
        (VIRTIO_MEM_RESP_ACK, 0)
    }
    pub(crate) fn process_state(&mut self, addr: u64, nb_blocks: u16) -> (u16, u16) {
        //info!("Memory.process_state");
        
        let size = nb_blocks as u64 * self.block_size();

        if !self.is_request_range_valid(addr, size) {
            //info!("State request invalid: addr {}, nb_blocks {}", addr, nb_blocks);
            return (VIRTIO_MEM_RESP_ERROR, 0);
        }

        let offset = addr - self.config_space.addr;
        let first_block_index = (offset / self.block_size()) as usize;
        
        let range_state = if self.memory_bitmap.is_range_state(first_block_index, nb_blocks, true) {
            VIRTIO_MEM_STATE_PLUGGED
        } else if self.memory_bitmap.is_range_state(first_block_index, nb_blocks, false) {
            VIRTIO_MEM_STATE_UNPLUGGED
        } else {
            VIRTIO_MEM_STATE_MIXED
        };

        (VIRTIO_MEM_RESP_ACK, range_state)
    }

    pub(crate) fn process_request_queue_event(&mut self) -> MemoryResult<()> {
        self.queue_evts[GUEST_REQUESTS_INDEX]
            .read()
            .map_err(MemoryError::EventFd)?;

        while let Some(head) = self.queues[GUEST_REQUESTS_INDEX].pop(self.device_state.mem().unwrap()) {
            if head.len as usize >= size_of::<VirtioMemReq>() {
                let head_addr = head.addr;
                let head_index = head.index;

                let req: VirtioMemReq = self.device_state.mem().unwrap()
                    .read_obj(head_addr)
                    .map_err(|_| MemoryError::GuestMemory)?;

                info!("virtio-mem request received: {}", req.req_type);

                let (resp_type, resp_state) = match req.req_type {
                        VIRTIO_MEM_REQ_PLUG => {
                            info!("VIRTIO_MEM_REQ_PLUG");
                            self.process_plug(req.data.addr, req.data.nb_blocks)
                        }
                        
                        VIRTIO_MEM_REQ_UNPLUG => {
                            //info!("VIRTIO_MEM_REQ_UNPLUG");
                            self.process_unplug(req.data.addr, req.data.nb_blocks)
                        }
                        VIRTIO_MEM_REQ_UNPLUG_ALL => {
                            info!("VIRTIO_MEM_REQ_UNPLUG_ALL");
                            self.process_unplug_all()
                        }
                        VIRTIO_MEM_REQ_STATE => {
                            info!("Handling VIRTIO_MEM_REQ_STATE");
                            self.process_state(req.data.addr, req.data.nb_blocks)
                        }
                        _ => {
                            error!("Unknown request type: {}", req.req_type);
                            return Err(MemoryError::UnknownRequestType(req.req_type));
                        }    
                };

                let resp = VirtioMemResp::new(resp_type, resp_state);

                //info!("Memory sending response: {:?}", resp);
                self.device_state.mem().unwrap()
                    .write_obj(resp, head_addr)
                    .map_err(|_| MemoryError::ResponseSendFailure)?;
                self.queues[GUEST_REQUESTS_INDEX]
                    .add_used(self.device_state.mem().unwrap(), head_index, size_of::<VirtioMemResp>() as u32)
                    .map_err(MemoryError::Queue)?;
            } else {
                error!("Invalid request size: expected at least {} bytes", std::mem::size_of::<VirtioMemReq>());
                return Err(MemoryError::GuestMemory);
            }
        }

        self.irq_trigger
            .trigger_irq(IrqType::Vring)
            .map_err(MemoryError::InterruptError)?;

        Ok(())
    }   

    #[inline]
    fn addr_is_set(&self) -> bool {
        self.addr_is_set
    }

    /// Set the start address of the memory region managed by this device.
    pub fn set_addr(&mut self, addr: u64) -> MemoryResult<()> {
        if self.addr_is_set() {
            return Err(MemoryError::AddressAlreadySet);
        }
        self.config_space.addr = addr;
        self.addr_is_set = true;
        Ok(())
    }

    fn host_addr_is_set(&self) -> bool {
        self.host_addr_is_set
    }

    pub fn set_host_addr(&mut self, addr: u64) -> MemoryResult<()> {
        if self.host_addr_is_set() {
            return Err(MemoryError::HostAddressAlreadySet);
        }
        self.host_addr = addr;
        self.host_addr_is_set = true;
        Ok(())
    }

    /// Returns the identifier of the device.
    pub fn id(&self) -> &str {
        self.id.as_str()
    }
    /// Returns the region size (in Bytes) of the device.
    pub fn region_size(&self) -> u64 {
        self.config_space.region_size
    }
    /// Returns the block size (in Bytes) of the device.
    pub fn block_size(&self) -> u64 {
        self.config_space.block_size
    }

    pub fn requested_size(&self) -> u64 {
        self.config_space.requested_size
    }

    pub fn node_id(&self) -> u16 {
        self.config_space.node_id
    }

    /// Handle
    pub fn change_requested_size(&mut self, requested_size_kib: u64) -> MemoryResult<()> {
        info!(
            "Got a request to change the requested_size of memory device [{}] to [{}] kbytes",
            self.id(),
            requested_size_kib
        );
        if self.is_activated() {
            let requested_size = requested_size_kib * KIB;
            if self.config_space.usable_region_size < requested_size {
                return Err(MemoryError::RequestedSizeTooLarge);
            }
            if requested_size % self.block_size() != 0 {
                return Err(MemoryError::SizeNotMultipleOfBlockSize);
            }

            self.config_space.requested_size = requested_size;
            self.irq_trigger
                .trigger_irq(IrqType::Config)
                .map_err(MemoryError::InterruptError)
        } else {
            Err(MemoryError::DeviceNotActive)
        }
    }

    /// Get the configuration of the memory device.
    pub fn config(&self) -> MemoryConfig {
        info!("config");
        MemoryConfig {
            id: self.id.clone(),
            block_size_kib: (self.block_size() / KIB),
            node_id: Some(self.node_id()),
            region_size_kib: (self.region_size() / KIB),
            requested_size_kib: (self.requested_size() / KIB),
        }
    }
}
impl VirtioDevice for Memory {
    fn device_type(&self) -> u32 {
        TYPE_MEMORY
    }
    fn queues(&self) -> &[Queue] {
        &self.queues
    }
    fn queues_mut(&mut self) -> &mut [Queue] {
        &mut self.queues
    }
    fn queue_events(&self) -> &[EventFd] {
        &self.queue_evts
    }

    fn interrupt_trigger(&self) -> &IrqTrigger {
        &self.irq_trigger
    }
    /* 
    fn interrupt_status(&self) -> Arc<AtomicUsize> {
        self.irq_trigger.irq_status.clone()
    } */
    fn avail_features(&self) -> u64 {
        self.avail_features
    }
    fn acked_features(&self) -> u64 {
        self.acked_features
    }
    fn set_acked_features(&mut self, acked_features: u64) {
        self.acked_features = acked_features;
    }
    fn read_config(&self, offset: u64, mut data: &mut [u8]) {
        let config_space_bytes = self.config_space.as_slice();
        let config_len = config_space_bytes.len() as u64;
        if offset >= config_len {
            error!("Failed to read config space");
            return;
        }
        if let Some(end) = offset.checked_add(data.len() as u64) {
            // This write can't fail, offset and end are checked against config_len.
            data.write_all(
                &config_space_bytes[offset as usize..cmp::min(end, config_len) as usize],
            )
            .unwrap();
        }
    }
    fn write_config(&mut self, offset: u64, data: &[u8]) {
        let data_len = data.len() as u64;
        let config_space_bytes = self.config_space.as_mut_slice();
        let config_len = config_space_bytes.len() as u64;
        if offset + data_len > config_len {
            error!("Failed to write config space");
            return;
        }
        config_space_bytes[offset as usize..(offset + data_len) as usize].copy_from_slice(data);
    }
    fn is_activated(&self) -> bool {
        self.device_state.is_activated()
    }
    fn activate(&mut self, mem: GuestMemoryMmap) -> Result<(), ActivateError> {
        info!("Memory.activate");
        if self.activate_evt.write(1).is_err() {
            info!("Memory: Cannot write to activate_evt");
            // TODO: Increment metrics (?)
            self.device_state = DeviceState::Inactive;
            return Err(ActivateError::EventFd);
        }
        
        self.device_state = DeviceState::Activated(mem);
        info!("Memory device [{}] got activated!", self.id());
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use utils::{byte_order, get_page_size};
    use crate::devices::virtio::gen::virtio_blk::VIRTIO_F_VERSION_1;
    use vm_memory::ByteValued;
    use super::super::CONFIG_SPACE_SIZE;
    use super::{Memory, MemoryError, TYPE_MEMORY};
    use crate::devices::virtio::memory::device::ConfigSpace;
    use crate::devices::virtio::queue::Queue;
    use crate::devices::virtio::device::VirtioDevice;
    impl Memory {
        pub(crate) fn set_queue(&mut self, idx: usize, q: Queue) {
            self.queues[idx] = q;
        }
    }
    
    fn page_size() -> u64 {
        get_page_size().unwrap() as u64
    }

    #[test]
    fn test_new_memory() {
        let page_size: u64 = page_size();
        let block_size_zero = Memory::new(0, None, 0, String::from("memory-dev-1"), 0);
        match block_size_zero {
            Err(MemoryError::BlockSizeIsZero) => {}
            _ => unreachable!(),
        }
        let block_size_allignment =
            Memory::new(page_size + 1, None, 0, String::from("memory-dev-2"), 0);
        match block_size_allignment {
            Err(MemoryError::BlockSizeNotMultipleOfPageSize(_)) => {}
            _ => unreachable!(),
        }
        let block_size_power2 = Memory::new(page_size * 3, None, 0, String::from("memory-dev-3"), 0);
        match block_size_power2 {
            Err(MemoryError::BlockSizeNotPowerOf2) => {}
            _ => unreachable!(),
        }
        let region_size_multiple =
            Memory::new(page_size, None, page_size + 1, String::from("memory-dev-4"), 0);
        match region_size_multiple {
            Err(MemoryError::SizeNotMultipleOfBlockSize) => {}
            _ => unreachable!(),
        }
        let memory_ok =
            Memory::new(page_size, None, page_size, String::from("memory-dev-5"), 0).unwrap();
        assert_eq!(memory_ok.device_type(), TYPE_MEMORY);
        assert_eq!(memory_ok.id(), "memory-dev-5");
        assert!(!memory_ok.addr_is_set());
        assert_eq!(memory_ok.acked_features(), 0);
        assert_eq!(memory_ok.avail_features(), 1u64 << VIRTIO_F_VERSION_1);
        assert_eq!(memory_ok.block_size(), page_size);
        assert_eq!(memory_ok.region_size(), page_size);
    }
    #[test]
    fn test_read_config() {
        let page_size: u64 = get_page_size().unwrap() as u64;
        let memory_device =
            Memory::new(page_size, None, page_size, String::from("memory-dev"), 0).unwrap();
        // 7 fields of 8 Bytes each
        let mut actual_config_space = [0u8; CONFIG_SPACE_SIZE];
        memory_device.read_config(0, &mut actual_config_space);
        // pub block_size: u64,
        assert_eq!(
            byte_order::read_le_u64(&actual_config_space[..8]),
            page_size
        );
        // pub node_id: u16,
        assert_eq!(byte_order::read_le_u16(&actual_config_space[8..10]), 0u16);
        // _padding: [u8; 6],
        assert_eq!(&actual_config_space[10..16], [0u8; 6]);
        // pub addr: u64,
        assert_eq!(byte_order::read_le_u64(&actual_config_space[16..24]), 0u64);
        // pub region_size: u64,
        assert_eq!(
            byte_order::read_le_u64(&actual_config_space[24..32]),
            page_size
        );
        // pub usable_region_size: u64,
        assert_eq!(byte_order::read_le_u64(&actual_config_space[32..40]), 0u64);
        // pub plugged_size: u64,
        assert_eq!(byte_order::read_le_u64(&actual_config_space[40..41]), 0u64);
        // pub requested_size: u64,
        assert_eq!(
            byte_order::read_le_u64(&actual_config_space[48..CONFIG_SPACE_SIZE]),
            0u64
        );
    }
    #[test]
    fn test_write_config() {
        let page_size: u64 = get_page_size().unwrap() as u64;
        let mut memory_device =
            Memory::new(page_size, None, page_size, String::from("memory-dev"), 0).unwrap();
        let mut expected_config_space: [u8; CONFIG_SPACE_SIZE] = [0u8; CONFIG_SPACE_SIZE];
        // reading the expected config is assured by `test_read_config()`
        memory_device.read_config(0, &mut expected_config_space);
        // this write should fail to write
        let garbage_config_space = [0xffu8; CONFIG_SPACE_SIZE];
        memory_device.write_config(3, &garbage_config_space);
        // reading again
        let mut actual_config_space = [0xffu8; CONFIG_SPACE_SIZE];
        memory_device.read_config(0, &mut actual_config_space);
        // this write should go through setting every bit
        memory_device.write_config(0, &garbage_config_space);
        memory_device.read_config(0, &mut actual_config_space);
        assert_eq!(garbage_config_space, actual_config_space);
        // now construction of a valid config space
        let config_space = ConfigSpace {
            block_size: page_size * 2,
            node_id: 0,
            _padding: [0u8; 6],
            addr: 0,
            region_size: page_size * 8,
            usable_region_size: 0,
            plugged_size: 0,
            requested_size: 0,
        };
        // Writing should go through
        let new_config_space = config_space.as_slice();
        memory_device.write_config(0, &new_config_space);
        assert_eq!(memory_device.block_size(), page_size * 2);
        assert_eq!(memory_device.region_size(), page_size * 8);
    }
}