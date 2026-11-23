//! Windows: the efficiency class of every logical processor.

use windows_sys::Win32::System::SystemInformation::{GetLogicalProcessorInformationEx, RelationProcessorCore, SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX};

/// Each logical processor's efficiency class, by index (group by
/// group, 64 a group). Empty when Windows does not say.
pub fn efficiency_classes() -> Vec<u8> {
    let mut len = 0u32;
    // SAFETY: a null buffer asks only for the size needed.
    unsafe { GetLogicalProcessorInformationEx(RelationProcessorCore, std::ptr::null_mut(), &mut len) };
    if len == 0 {
        return Vec::new();
    }
    // u64 words keep the records aligned.
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: the buffer holds `len` bytes.
    if unsafe { GetLogicalProcessorInformationEx(RelationProcessorCore, buf.as_mut_ptr().cast(), &mut len) } == 0 {
        return Vec::new();
    }
    let bytes = buf.as_ptr().cast::<u8>();
    let mut classes: Vec<Option<u8>> = Vec::new();
    let mut offset = 0usize;
    // A record is often shorter than the struct (its union's largest
    // variant), so only its header and its own Size are checked.
    while offset + 8 <= len as usize {
        // SAFETY: Windows wrote a whole record at `offset` (records are
        // 8-byte aligned and sized); only its header and the Processor
        // variant it holds are read.
        let info = unsafe { &*bytes.add(offset).cast::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>() };
        if info.Size == 0 || offset + info.Size as usize > len as usize {
            break;
        }
        if info.Relationship == RelationProcessorCore {
            // SAFETY: a processor-core record holds the Processor variant.
            let core = unsafe { &info.Anonymous.Processor };
            let masks = unsafe { std::slice::from_raw_parts(core.GroupMask.as_ptr(), core.GroupCount as usize) };
            for m in masks {
                for bit in 0..usize::BITS as usize {
                    if m.Mask & (1usize << bit) != 0 {
                        let index = m.Group as usize * 64 + bit;
                        if classes.len() <= index {
                            classes.resize(index + 1, None);
                        }
                        classes[index] = Some(core.EfficiencyClass);
                    }
                }
            }
        }
        offset += info.Size as usize;
    }
    // A gap means indices we cannot place: then ignore the classes.
    classes.into_iter().collect::<Option<Vec<u8>>>().unwrap_or_default()
}
