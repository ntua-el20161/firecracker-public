// Copyright 2022 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
pub mod device;
pub mod event_handler;
pub use self::device::{ Memory, MemoryConfig };
use super::queue::QueueError;
pub const QUEUE_SIZE: u16 = 256;
// the index of guest requests queue from Memory device queues/queues_evts vector.
pub const GUEST_REQUESTS_INDEX: usize = 0;
pub const CONFIG_SPACE_SIZE: usize = 56;

// The feature bitmap for virtio memory.
const _VIRTIO_MEM_F_ACPI_PXM: u32 = 0; // The node id is valid and corresponds to an ACPI PXM.
const _VIRTIO_MEM_F_UNPLUGGED_INACCESSIBLE: u32 = 1; // The driver is not allowed to access unplugged memory.

// Virtio-mem request types
const VIRTIO_MEM_REQ_PLUG: u16 = 0;
const VIRTIO_MEM_REQ_UNPLUG: u16 = 1;
const VIRTIO_MEM_REQ_UNPLUG_ALL: u16 = 2;
const VIRTIO_MEM_REQ_STATE: u16 = 3;

// Virtio-mem response types
const VIRTIO_MEM_RESP_ACK: u16 = 0;
const VIRTIO_MEM_RESP_NACK: u16 = 1;
const VIRTIO_MEM_RESP_BUSY: u16 = 2;
const VIRTIO_MEM_RESP_ERROR: u16 = 3;

// Virtio-mem state types
const VIRTIO_MEM_STATE_PLUGGED: u16 = 0;
const VIRTIO_MEM_STATE_UNPLUGGED: u16 = 1;
const VIRTIO_MEM_STATE_MIXED: u16 = 2;

#[derive(Debug, thiserror::Error, displaydoc::Display)]
pub enum MemoryDeviceError {
    /// Activation error.
    Activate(super::ActivateError),
    /// Start address already set
    AddressAlreadySet,
    /// Bitmap is not initializeds
    BitmapNotPresent,
    /// Block Size is zero bytes.
    BlockSizeIsZero,
    /// Block Size not a multiple of page size.
    BlockSizeNotMultipleOfPageSize(u64),
    /// Block Size not a power of 2.
    BlockSizeNotPowerOf2,
    /// Device not activated yet.
    DeviceNotActive,
    /// No memory device found.
    DeviceNotFound,
    /// EventFd error.
    EventFd(std::io::Error),
    /// Guest Memmory Error
    GuestMemory,
    /// Host region start address already set.
    HostAddressAlreadySet,
    /// Error while sending an interrupt
    InterruptError(std::io::Error),
    /// Quereying page size error.
    PageSize(utils::errno::Error),
    /// Error while processing the virtq
    Queue(QueueError),
    /// Requested size exceed the size of the usable region.
    RequestedSizeTooLarge,
    /// Failed to send response to the guest.
    ResponseSendFailure,
    /// Size is not a multiple of Block Size.
    SizeNotMultipleOfBlockSize,
    /// Request type is none of the supported types.
    UnknownRequestType(u16),
} 

pub type MemoryResult<T> = std::result::Result<T, MemoryDeviceError>;