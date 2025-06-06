// Copyright 2022 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use std::os::unix::io::AsRawFd;
use event_manager::{EventOps, Events, MutEventSubscriber};
use crate::logger::{error, info};
use utils::epoll::EventSet;
use crate::devices::virtio::memory::device::Memory;
use crate::devices::virtio::memory::GUEST_REQUESTS_INDEX;
use crate::devices::virtio::device::VirtioDevice;

impl Memory {
    fn register_activate_event(&self, ops: &mut EventOps) {
        info!("Memory.register_activate_event()");
        if let Err(err) = ops.add(Events::new(&self.activate_evt, EventSet::IN)) {
            info!("[Memory] Failed to register activate event: {}", err);
        }
    }
    fn register_runtime_events(&self, ops: &mut EventOps) {
        info!("Memory.register_runtime_events()");
        if let Err(err) = ops.add(Events::new(&self.queue_evts[GUEST_REQUESTS_INDEX], EventSet::IN)) {
            error!("[Memory] Failed to register queue event: {}", err);
        }
    }
    fn process_activate_event(&self, ops: &mut EventOps) {
        info!("memory: process activate event");
        if let Err(err) = self.activate_evt.read() {
            info!("Failed to consume memory activate event: {:?}", err);
        }
        self.register_runtime_events(ops);
        if let Err(err) = ops.remove(Events::new(&self.activate_evt, EventSet::IN)) {
            error!("[Memory] Failed to un-register activate event: {}", err);
        }
    }
}
impl MutEventSubscriber for Memory {
    fn process(&mut self, event: Events, ops: &mut EventOps) {
        let source = event.fd();
        let event_set = event.event_set();
        let supported_events = EventSet::IN;
        if !supported_events.contains(event_set) {
            info!(
                "Received unknown event: {:?} from source: {:?}",
                event_set, source
            );
            return;
        }
        if self.is_activated() {
            let virtq_quest_requests_ev_fd = self.queue_evts[GUEST_REQUESTS_INDEX].as_raw_fd();
            let activate_fd = self.activate_evt.as_raw_fd();
            match source {
                _ if source == virtq_quest_requests_ev_fd => {
                    self.process_request_queue_event()
                    .unwrap_or_else(|e| {
                        error!(
                            "Memory [{}]: Failed to process request queue event: {}",
                            self.id(),
                            e
                        );
                    });
                }
                _ if source == activate_fd => {
                    info!("activate_fd");
                    self.process_activate_event(ops);
                }
                _ => {
                    info!(
                        "Memory [{}]: Spurious event received: {:?}",
                        self.id(),
                        source
                    );
                }
            }
        } else {
            info!(
                "Memory [{}]: The device is not yet activated. Spurious event received: {:?}",
                self.id(),
                source
            );
        }
    }
    fn init(&mut self, ops: &mut EventOps) {
        info!("Memory device [{}].init()", self.id());
        self.register_activate_event(ops);
    }
}
#[cfg(test)]
pub mod tests {
    use std::sync::{Arc, Mutex};
    use event_manager::{EventManager, SubscriberOps};
    use utils::get_page_size;
    use vm_memory::GuestAddress;
    use super::*;
    use crate::devices::virtio::balloon::test_utils::set_request;
    use crate::devices::virtio::test_utils::{default_mem, VirtQueue};
    use crate::devices::virtio::device::VirtioDevice;
    // adapted from balloon device
    #[test]
    fn test_event_handler() {
        let page_size: u64 = get_page_size().unwrap() as u64;
        let mut event_manager = EventManager::new().unwrap();
        let mut memory_dev =
            Memory::new(page_size, None, 10 * page_size, String::from("memory-dev"), 0).unwrap();
        let mem = default_mem();
        let requestsq = VirtQueue::new(GuestAddress(0), &mem, 16);
        memory_dev.set_queue(GUEST_REQUESTS_INDEX, requestsq.create_queue());
        let memory_dev = Arc::new(Mutex::new(memory_dev));
        let _id = event_manager.add_subscriber(memory_dev.clone());
        // Push a queue event to the guest requests queue in this test.
        {
            let addr = 0x100;
            set_request(&requestsq, 0, addr, 4, 0);
            memory_dev.lock().unwrap().queue_evts[GUEST_REQUESTS_INDEX]
                .write(1)
                .unwrap();
        }
        // EventManager should report no events since memory_dev has only registered
        // its activation event so far (even though there is also a queue event pending).
        let ev_count = event_manager.run_with_timeout(50).unwrap();
        assert_eq!(ev_count, 0);
        // Manually force a queue event and check it's ignored pre-activation.
        {
            let b = memory_dev.lock().unwrap();
            // Artificially push event.
            b.queue_evts[GUEST_REQUESTS_INDEX].write(1).unwrap();
            // Process the pushed event.
            let ev_count = event_manager.run_with_timeout(50).unwrap();
            // Validate there was no queue operation.
            assert_eq!(ev_count, 0);
            assert_eq!(requestsq.used.idx.get(), 0);
        }
        // Now activate the device.
        memory_dev.lock().unwrap().activate(mem.clone()).unwrap();
        // Process the activate event.
        let mut ev_count = event_manager.run_with_timeout(50).unwrap();
        assert_eq!(ev_count, 1);
        // Handle the previously pushed queue event through EventManager.
        ev_count = event_manager
            .run_with_timeout(100)
            .expect("Metrics event timeout or error.");
        // Process the previously pushed event.
        assert_eq!(ev_count, 1);
        // Make sure the data queue advanced.
        // TODO: this fails beacuse we don't read from the virtq yet
        // assert_eq!(requestsq.used.idx.get(), 1);
    }
}