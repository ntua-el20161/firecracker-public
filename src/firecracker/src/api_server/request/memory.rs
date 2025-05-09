use super::super::parsed_request::{ParsedRequest, RequestError};
use vmm::rpc_interface::VmmAction;
use vmm::logger::info;
use vmm::vmm_config::memory::{ MemoryDeviceConfig, MemoryUpdateConfig };
use super::Body;

pub(crate) fn parse_get_memory() -> Result<ParsedRequest, RequestError> {
    info!("parse_get_memory");
    Ok(ParsedRequest::new_sync(VmmAction::GetVirtioMemConfig))
}

pub(crate) fn parse_patch_memory(
    body: &Body,
) -> Result<ParsedRequest, RequestError> {
    info!("parse_patch_memory");
    Ok(ParsedRequest::new_sync(VmmAction::UpdateMemoryDevice(
        serde_json::from_slice::<MemoryUpdateConfig>(body.raw())?,
    )))
}