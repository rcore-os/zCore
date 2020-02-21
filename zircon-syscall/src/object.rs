use super::*;

const ZX_PROP_NAME: u32 = 3;
const ZX_MAX_NAME_LEN: u32 = 32;

impl Syscall {
    pub fn sys_object_get_property(
        &self,
        handle_value: HandleValue,
        property: u32,
        mut ptr: UserOutPtr<u8>,
        buffer_size: u32,
    ) -> ZxResult<usize> {
        info!(
            "handle={:?}, property={:?}, buffer_ptr={:?}, size={:?}",
            handle_value, property, ptr, buffer_size
        );
        let object = self
            .thread
            .proc()
            .get_dyn_object_with_rights(handle_value, Rights::GET_PROPERTY)?;
        match property {
            ZX_PROP_NAME => {
                if buffer_size < ZX_MAX_NAME_LEN {
                    Err(ZxError::BUFFER_TOO_SMALL)
                } else {
                    let s = object.name();
                    info!("object_get_property: name is {}", s);
                    ptr.write_cstring(s.as_str())
                        .expect("failed to write cstring");
                    Ok(0)
                }
            }
            _ => {
                warn!("unknown property {} in OBJECT_GET_PROPERTY", property);
                Err(ZxError::INVALID_ARGS)
            }
        }
    }

    pub fn sys_object_set_property(
        &self,
        handle_value: HandleValue,
        property: u32,
        ptr: UserInPtr<u8>,
        buffer_size: u32,
    ) -> ZxResult<usize> {
        info!(
            "handle={:?}, property={:?}, buffer_ptr={:?}, size={:?}",
            handle_value, property, ptr, buffer_size
        );
        let object = self
            .thread
            .proc()
            .get_dyn_object_with_rights(handle_value, Rights::SET_PROPERTY)?;
        match property {
            ZX_PROP_NAME => {
                let length = if buffer_size > ZX_MAX_NAME_LEN {
                    (ZX_MAX_NAME_LEN - 1) as usize
                } else {
                    buffer_size as usize
                };
                let s = ptr.read_string(length)?;
                info!("object_set_property name: {}", s);
                object.set_name(&s);
                Ok(0)
            }
            _ => {
                warn!("unknown property {} in OBJECT_GET_PROPERTY", property);
                Err(ZxError::INVALID_ARGS)
            }
        }
    }
}
