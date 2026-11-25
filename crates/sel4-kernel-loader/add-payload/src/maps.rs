//
// Copyright 2026, Colias Group, LLC
//
// SPDX-License-Identifier: BSD-2-Clause
//

#![allow(unused_variables)]
#![allow(dead_code)]

use std::ops::Range;

use sel4_platform_info_types::OwnedPlatformInfo;

use crate::page_tables::{
    LeafDescriptor, MkLeafArgs, RawDescriptor, Region, RegionsBuilder, Scheme, schemes,
};

pub(crate) fn mk_loader_map(
    scheme: &Scheme,
    smp: bool,
    vaddr: u64,
    platform_info: &OwnedPlatformInfo,
) -> (Vec<u8>, u64) {
    let device_range_end = match scheme {
        Scheme::AArch64 => 1 << 39,
        Scheme::AArch32 => scheme.virt_bounds().end,
        _ => panic!(),
    };

    let mut regions = RegionsBuilder::new(scheme);
    regions = regions.insert(Region::valid(
        0..device_range_end,
        mk_device_leaf_for_loader_map,
    ));
    for range in platform_info.memory.iter() {
        regions = regions.insert(Region::valid(range.clone(), move |args| {
            mk_normal_leaf_for_loader_map(smp, args)
        }));
    }

    regions
        .build()
        .construct_table(scheme)
        .embed(scheme, vaddr, false)
}

pub(crate) fn mk_kernel_map(
    scheme: &Scheme,
    smp: bool,
    vaddr: u64,
    kernel_virt_addr_range: Range<u64>,
    kernel_phys_to_virt_offset: u64,
    thead_maee: bool,
    platform_info: &OwnedPlatformInfo,
) -> (Vec<u8>, u64) {
    let virt_start = kernel_virt_addr_range.start;
    let virt_end = kernel_virt_addr_range.end;
    let virt_map_end = virt_end.next_multiple_of(1 << scheme.largest_leaf_size_bits());

    // The loader runs its last instructions from the identity mapping, at
    // its own address in platform memory. On AArch64 nothing else in that
    // mapping is ever executed, so the rest is execute-never; see
    // `mk_device_leaf_for_loader_map`.
    let executable_everywhere = !matches!(scheme, Scheme::AArch64);
    let mut regions = RegionsBuilder::new(scheme)
        .insert(Region::valid(0..virt_start, move |loc| {
            mk_identity_leaf_for_kernel_map(thead_maee, executable_everywhere, loc)
        }));
    if !executable_everywhere {
        for range in platform_info.memory.iter() {
            let range = range.start..range.end.min(virt_start);
            if range.start < range.end {
                regions = regions.insert(Region::valid(range, move |loc| {
                    mk_identity_leaf_for_kernel_map(thead_maee, true, loc)
                }));
            }
        }
    }
    let regions = regions.insert(Region::valid(virt_start..virt_map_end, move |loc| {
        mk_kernel_leaf_for_kernel_map(smp, kernel_phys_to_virt_offset, thead_maee, loc)
    }));

    regions
        .build()
        .construct_table(scheme)
        .embed(scheme, vaddr, thead_maee)
}

fn mk_normal_leaf_for_loader_map(smp: bool, loc: MkLeafArgs) -> RawDescriptor {
    match loc.scheme() {
        Scheme::AArch64 => {
            loc.identity_descriptor::<schemes::AArch64LeafDescriptor>()
                .set_access_flag(true)
                .set_attribute_index(4) // select MT_NORMAL
                .set_shareability(aarch64_normal_shareability(smp))
                .to_raw()
        }
        Scheme::AArch32 => loc
            .identity_descriptor::<schemes::AArch32LeafDescriptor>()
            .set_access_flag(true)
            .set_attributes(0b101, false, true)
            .set_shareability(true)
            .to_raw(),
        _ => panic!(),
    }
}

fn mk_device_leaf_for_loader_map(loc: MkLeafArgs) -> RawDescriptor {
    match loc.scheme() {
        // Execute-never: the loader runs from platform memory only, and a
        // device mapping that may be executed lets the core fetch
        // instructions from it speculatively. On a board whose firmware
        // guards part of the address space, such a fetch is a violation the
        // firmware answers with a reset: the Radxa Dragon Q6A reset within
        // 200 ms of the loader turning its translation on.
        Scheme::AArch64 => loc
            .identity_descriptor::<schemes::AArch64LeafDescriptor>()
            .set_access_flag(true)
            .set_attribute_index(0)
            .set_execute_never(true)
            .to_raw(),
        Scheme::AArch32 => loc
            .identity_descriptor::<schemes::AArch32LeafDescriptor>()
            .set_access_flag(true)
            .to_raw(),
        _ => panic!(),
    }
}

fn mk_identity_leaf_for_kernel_map(
    thead_maee: bool,
    executable: bool,
    loc: MkLeafArgs,
) -> RawDescriptor {
    match loc.scheme() {
        Scheme::AArch64 => loc
            .identity_descriptor::<schemes::AArch64LeafDescriptor>()
            .set_access_flag(true)
            .set_attribute_index(0) // select MT_DEVICE_nGnRnE
            .set_execute_never(!executable)
            .to_raw(),
        Scheme::AArch32 => loc
            .identity_descriptor::<schemes::AArch32LeafDescriptor>()
            .set_access_flag(true)
            .to_raw(),
        Scheme::RiscVSv39 | Scheme::RiscVSv32 => {
            let raw = loc
                .identity_descriptor::<schemes::RiscVLeafDescriptor>()
                .to_raw();
            if thead_maee {
                raw | (0b0_1110_u64 << 59)
            } else {
                raw
            }
        }
    }
}

fn mk_kernel_leaf_for_kernel_map(
    smp: bool,
    phys_to_virt_offset: u64,
    thead_maee: bool,
    loc: MkLeafArgs,
) -> RawDescriptor {
    let f = |vaddr: u64| vaddr.wrapping_sub(phys_to_virt_offset);
    match loc.scheme() {
        Scheme::AArch64 => loc
            .descriptor::<schemes::AArch64LeafDescriptor>(f)
            .set_access_flag(true)
            .set_attribute_index(4) // select MT_NORMAL
            .set_shareability(aarch64_normal_shareability(smp))
            .to_raw(),
        Scheme::AArch32 => loc
            .descriptor::<schemes::AArch32LeafDescriptor>(f)
            .set_access_flag(true)
            .set_shareability(true)
            .to_raw(),
        Scheme::RiscVSv39 | Scheme::RiscVSv32 => {
            let raw = loc.descriptor::<schemes::RiscVLeafDescriptor>(f).to_raw();
            if thead_maee {
                // The physical gate observed sxstatus.MAEE=1 on this board.
                // C906 therefore interprets PTE[63:59] as XTheadMae attributes;
                // kernel executable mappings are normal cacheable memory.
                raw | (0b0_1110_u64 << 59)
            } else {
                raw
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const THEAD_PMA: u64 = 0b0_1110_u64 << 59;

    const AARCH64_XN: u64 = 1 << 54;

    #[test]
    fn aarch64_device_leaves_of_the_loader_map_are_execute_never() {
        let scheme = Scheme::AArch64;
        let device = MkLeafArgs::new_for_test(&scheme, 1, 0x4000_0000);
        let normal = MkLeafArgs::new_for_test(&scheme, 1, 0x1_0000_0000);

        assert_eq!(
            mk_device_leaf_for_loader_map(device) & AARCH64_XN,
            AARCH64_XN
        );
        assert_eq!(mk_normal_leaf_for_loader_map(false, normal) & AARCH64_XN, 0);
    }

    #[test]
    fn aarch64_identity_leaves_of_the_kernel_map_are_executable_only_on_request() {
        let scheme = Scheme::AArch64;
        let outside = MkLeafArgs::new_for_test(&scheme, 1, 0x4000_0000);
        let memory = MkLeafArgs::new_for_test(&scheme, 1, 0x1_0000_0000);

        assert_eq!(
            mk_identity_leaf_for_kernel_map(false, false, outside) & AARCH64_XN,
            AARCH64_XN
        );
        assert_eq!(
            mk_identity_leaf_for_kernel_map(false, true, memory) & AARCH64_XN,
            0
        );
    }

    #[test]
    fn thead_maee_marks_riscv_kernel_leaves() {
        let scheme = Scheme::RiscVSv39;
        let plain = MkLeafArgs::new_for_test(&scheme, 1, 0x8020_0000);
        let marked = MkLeafArgs::new_for_test(&scheme, 1, 0x8020_0000);
        let plain = mk_kernel_leaf_for_kernel_map(false, 0, false, plain);
        let marked = mk_kernel_leaf_for_kernel_map(false, 0, true, marked);

        assert_eq!(plain & THEAD_PMA, 0);
        assert_eq!(marked & THEAD_PMA, THEAD_PMA);
        assert_eq!(plain, marked & !THEAD_PMA);
    }
}

fn aarch64_normal_shareability(smp: bool) -> u64 {
    if smp { 0b11 } else { 0b00 }
}
