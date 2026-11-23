//! Fail closed before cleartext inference unless Windows has an up interface with a Tailscale address.
//! This checks local interface state, not peer identity or an OS route guarantee. Prefer HTTPS.

#![allow(unsafe_code)] // Validated, read-only traversal of Windows-owned layout in our buffer.

use std::{
    mem::{MaybeUninit, size_of},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    ptr::{addr_of, null},
};
use todo_client::assistant::local::{TailnetInterface, is_tailnet_ip};
use windows_sys::Win32::{
    Foundation::{ERROR_BUFFER_OVERFLOW, NO_ERROR},
    NetworkManagement::{
        IpHelper::{
            GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
            GetAdaptersAddresses, IP_ADAPTER_ADDRESSES_LH,
        },
        Ndis::IfOperStatusUp,
    },
    Networking::WinSock::{
        AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR, SOCKADDR_IN, SOCKADDR_IN6, SOCKET_ADDRESS,
    },
};

#[derive(Debug)]
pub struct WindowsTailnet;

impl TailnetInterface for WindowsTailnet {
    fn connected(&self) -> bool {
        addresses(is_tailnet_ip)
    }
}

fn addresses(mut matches: impl FnMut(IpAddr) -> bool) -> bool {
    const FLAGS: u32 = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut needed = 15 * 1024;
    for _ in 0..3 {
        let bytes = needed as usize;
        if bytes > 1024 * 1024 {
            return false;
        }
        // Use storage aligned for the API's first struct and keep it alive for the traversal.
        let mut storage = vec![
            MaybeUninit::<IP_ADAPTER_ADDRESSES_LH>::uninit();
            bytes.div_ceil(size_of::<IP_ADAPTER_ADDRESSES_LH>())
        ];
        let first = storage.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        // SAFETY: first points to at least `needed` writable bytes with the correct alignment.
        match unsafe {
            GetAdaptersAddresses(u32::from(AF_UNSPEC), FLAGS, null(), first, &mut needed)
        } {
            ERROR_BUFFER_OVERFLOW => continue,
            NO_ERROR => {}
            _ => return false,
        }
        // SAFETY: on success Windows returns a linked list of initialized adapter and address
        // records, with nullable links. All pointers are valid until our buffer is released.
        unsafe {
            let mut adapter = first.cast_const();
            while let Some(current) = adapter.as_ref() {
                if current.OperStatus == IfOperStatusUp {
                    let mut entry = current.FirstUnicastAddress.cast_const();
                    while let Some(address) = entry.as_ref() {
                        if ip(address.Address).is_some_and(&mut matches) {
                            return true;
                        }
                        entry = address.Next;
                    }
                }
                adapter = current.Next;
            }
        }
        return false;
    }
    false
}

/// The API owns the address type and byte count; only read fields covered by that count.
unsafe fn ip(socket: SOCKET_ADDRESS) -> Option<IpAddr> {
    let length = usize::try_from(socket.iSockaddrLength).ok()?;
    let wire = socket.lpSockaddr.cast_const();
    if wire.is_null() || length < size_of::<SOCKADDR>() {
        return None;
    }
    // SAFETY: the caller guarantees a valid OS socket address of at least length bytes.
    unsafe {
        match addr_of!((*wire).sa_family).read_unaligned() {
            AF_INET if length >= size_of::<SOCKADDR_IN>() => {
                let v4 = wire.cast::<SOCKADDR_IN>();
                Some(
                    Ipv4Addr::from(
                        addr_of!((*v4).sin_addr.S_un.S_addr)
                            .read_unaligned()
                            .to_ne_bytes(),
                    )
                    .into(),
                )
            }
            AF_INET6 if length >= size_of::<SOCKADDR_IN6>() => {
                let v6 = wire.cast::<SOCKADDR_IN6>();
                Some(Ipv6Addr::from(addr_of!((*v6).sin6_addr.u.Byte).read_unaligned()).into())
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_live_local_addresses_from_windows_without_network_requests() {
        assert!(addresses(|addr| addr.is_loopback()));
    }
}
