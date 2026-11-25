//! The network: how this computer finds and reaches its peer.
//!
//! The engine (`chat`) runs on this port and never touches iroh itself. The app uses
//! iroh over the LAN, found with mDNS. Tests use an in-memory network, so the protocol
//! runs in milliseconds and with time under the test's control.
//!
//! Each type is an enum with one variant per adapter. A connection carries
//! bidirectional streams: the session's frames on the first, files on the rest.

use std::{borrow::Cow, net::IpAddr, pin::Pin};

use anyhow::Result;
use iroh::{
    Endpoint, EndpointId, SecretKey, TransportAddr,
    address_lookup::AddrFilter,
    endpoint::{Incoming, PortmapperConfig, presets},
    endpoint_info::UserData,
};
use iroh_mdns_address_lookup::{DiscoveryEvent, MdnsAddressLookup};
use n0_future::{Stream, StreamExt};

/// What connections are for, so only other Tightbeam apps connect.
const ALPN: &[u8] = b"tightbeam/1";
/// mDNS service name, so we only see other Tightbeam apps (`_tightbeam._udp.local`).
const SERVICE: &str = "tightbeam";

pub(crate) enum Network {
    Iroh { endpoint: Endpoint, mdns: MdnsAddressLookup },
}

impl Network {
    /// iroh on the LAN. `loopback_only` binds to 127.0.0.1, for running two copies on one
    /// machine. (Windows refuses to send from a LAN address to that same address.)
    pub async fn iroh(secret: SecretKey, loopback_only: bool) -> Result<Self> {
        // LAN only for now: no relay servers, no public DNS, just mDNS. No UPnP either:
        // we never ask the router to open a public port, and we only ever advertise
        // private addresses.
        let mut builder = Endpoint::builder(presets::Minimal)
            .secret_key(secret)
            .alpns(vec![ALPN.to_vec()])
            .portmapper_config(PortmapperConfig::Disabled)
            .addr_filter(AddrFilter::new(move |addrs| {
                Cow::Owned(addrs.iter().filter(|a| is_lan(a, loopback_only)).cloned().collect())
            }));
        if loopback_only {
            builder = builder.clear_ip_transports().bind_addr("127.0.0.1:0")?;
        }
        let endpoint = builder.bind().await?;
        let mdns = MdnsAddressLookup::builder().service_name(SERVICE).build(endpoint.id())?;
        endpoint.address_lookup()?.add(mdns.clone());
        Ok(Self::Iroh { endpoint, mdns })
    }

    pub fn id(&self) -> EndpointId {
        match self {
            Self::Iroh { endpoint, .. } => endpoint.id(),
        }
    }

    /// The name others see next to this computer when they look for someone to pair with.
    pub fn advertise(&self, name: &str) {
        match self {
            Self::Iroh { endpoint, .. } => {
                let data =
                    if name.is_empty() { None } else { UserData::try_from(name.to_string()).ok() };
                endpoint.set_user_data_for_address_lookup(data);
            }
        }
    }

    /// Who comes and goes nearby.
    pub async fn discoveries(&self) -> Discoveries {
        match self {
            Self::Iroh { mdns, .. } => Discoveries::Iroh(Box::pin(mdns.subscribe().await)),
        }
    }

    pub async fn connect(&self, peer: EndpointId) -> Result<Conn> {
        match self {
            Self::Iroh { endpoint, .. } => Ok(Conn::Iroh(endpoint.connect(peer, ALPN).await?)),
        }
    }

    /// The next incoming connection, or `None` once the network is closed.
    pub async fn accept(&self) -> Option<Accepting> {
        match self {
            Self::Iroh { endpoint, .. } => endpoint.accept().await.map(Accepting::Iroh),
        }
    }

    /// Closes every connection, so the peer notices right away.
    pub async fn close(&self) {
        match self {
            Self::Iroh { endpoint, .. } => endpoint.close().await,
        }
    }
}

/// Someone nearby appeared, with their advertised name, or went away.
pub(crate) enum Discovery {
    Found { id: EndpointId, name: String },
    Lost(EndpointId),
}

pub(crate) enum Discoveries {
    Iroh(Pin<Box<dyn Stream<Item = DiscoveryEvent> + Send>>),
}

impl Discoveries {
    pub async fn next(&mut self) -> Option<Discovery> {
        match self {
            Self::Iroh(events) => loop {
                match events.next().await? {
                    DiscoveryEvent::Discovered { endpoint_info, .. } => {
                        let id = endpoint_info.endpoint_id;
                        let name = endpoint_info.data.user_data().map(|u| u.to_string());
                        return Some(Discovery::Found { id, name: name.unwrap_or_default() });
                    }
                    DiscoveryEvent::Expired { endpoint_id } => {
                        return Some(Discovery::Lost(endpoint_id));
                    }
                    _ => continue,
                }
            },
        }
    }
}

/// A connection on its way in.
pub(crate) enum Accepting {
    Iroh(Incoming),
}

impl Accepting {
    pub async fn connected(self) -> Result<Conn> {
        match self {
            Self::Iroh(incoming) => Ok(Conn::Iroh(incoming.await?)),
        }
    }
}

#[derive(Clone)]
pub(crate) enum Conn {
    Iroh(iroh::endpoint::Connection),
}

/// The route a connection takes: the peer's address and the round trip in milliseconds.
pub(crate) struct Route {
    pub address: String,
    pub rtt_ms: f64,
}

impl Conn {
    pub fn remote_id(&self) -> EndpointId {
        match self {
            Self::Iroh(conn) => conn.remote_id(),
        }
    }

    pub async fn open_bi(&self) -> Result<(SendStream, RecvStream)> {
        match self {
            Self::Iroh(conn) => {
                let (send, recv) = conn.open_bi().await?;
                Ok((SendStream::Iroh(send), RecvStream::Iroh(recv)))
            }
        }
    }

    pub async fn accept_bi(&self) -> Result<(SendStream, RecvStream)> {
        match self {
            Self::Iroh(conn) => {
                let (send, recv) = conn.accept_bi().await?;
                Ok((SendStream::Iroh(send), RecvStream::Iroh(recv)))
            }
        }
    }

    /// Ends the connection for both sides; `reason` is for logs.
    pub fn close(&self, code: u32, reason: &[u8]) {
        match self {
            Self::Iroh(conn) => conn.close(code.into(), reason),
        }
    }

    /// Resolves once the connection has ended, from either side.
    pub async fn closed(&self) {
        match self {
            Self::Iroh(conn) => drop(conn.closed().await),
        }
    }

    pub fn route(&self) -> Option<Route> {
        match self {
            Self::Iroh(conn) => {
                let paths = conn.paths();
                let path = paths.iter().find(|p| p.is_selected())?;
                let (address, rtt) = (path.remote_addr().to_string(), path.rtt());
                Some(Route { address, rtt_ms: rtt.as_secs_f64() * 1000.0 })
            }
        }
    }
}

pub(crate) enum SendStream {
    Iroh(iroh::endpoint::SendStream),
}

impl SendStream {
    pub async fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        match self {
            Self::Iroh(send) => Ok(send.write_all(buf).await?),
        }
    }

    /// No more to send: the other side reads to the end.
    pub fn finish(&mut self) -> Result<()> {
        match self {
            Self::Iroh(send) => Ok(send.finish()?),
        }
    }

    /// Resolves once the other side has read everything, or stopped reading.
    pub async fn stopped(&mut self) {
        match self {
            Self::Iroh(send) => drop(send.stopped().await),
        }
    }
}

pub(crate) enum RecvStream {
    Iroh(iroh::endpoint::RecvStream),
}

impl RecvStream {
    /// Some bytes into `buf`, or `None` at the end of the stream.
    pub async fn read(&mut self, buf: &mut [u8]) -> Result<Option<usize>> {
        match self {
            Self::Iroh(recv) => Ok(recv.read(buf).await?),
        }
    }

    pub async fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
        match self {
            Self::Iroh(recv) => Ok(recv.read_exact(buf).await?),
        }
    }
}

/// Whether an address is on the local network (or loopback, when allowed).
fn is_lan(addr: &TransportAddr, allow_loopback: bool) -> bool {
    match addr {
        TransportAddr::Ip(sock) => match sock.ip() {
            IpAddr::V4(ip) if ip.is_loopback() => allow_loopback,
            IpAddr::V4(ip) => ip.is_private() || ip.is_link_local(),
            IpAddr::V6(ip) => ip.is_unique_local() || ip.is_unicast_link_local(),
        },
        _ => false,
    }
}
