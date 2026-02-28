use std::collections::HashMap;
use std::env;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const DEFAULT_MEDIA_PORT: u16 = 9292;
const DISCOVERY_PORT: u16 = 39292;
const DISCOVERY_MAGIC: &str = "KAGAMI_DISCOVERY";
const DISCOVERY_VERSION: &str = "1";
const BROADCAST_INTERVAL: Duration = Duration::from_secs(1);
const PEER_TTL: Duration = Duration::from_secs(5);

#[derive(Clone, Debug)]
pub struct Peer {
    pub id: String,
    pub display_name: String,
    pub addr: SocketAddr,
    pub capabilities: Vec<String>,
    pub last_seen_at: Instant,
}

impl Peer {
    pub fn label(&self) -> String {
        let short_id = self.id.chars().take(8).collect::<String>();
        let capabilities = if self.capabilities.is_empty() {
            "No capabilities".to_owned()
        } else {
            self.capabilities.join(", ")
        };

        format!(
            "{} · {} · {} · {}",
            self.display_name, self.addr, short_id, capabilities
        )
    }
}

pub struct DiscoveryEngine {
    instance_id: String,
    display_name: String,
    service_port: u16,
    capabilities: Vec<String>,
    peers: Arc<Mutex<HashMap<String, Peer>>>,
    running: Arc<AtomicBool>,
    broadcaster: Option<JoinHandle<()>>,
    listener: Option<JoinHandle<()>>,
}

impl DiscoveryEngine {
    pub fn new(service_port: u16, capabilities: Vec<String>) -> Self {
        let display_name = env::var("HOSTNAME")
            .or_else(|_| env::var("COMPUTERNAME"))
            .or_else(|_| env::var("USER"))
            .unwrap_or_else(|_| "Kagami".to_owned());
        let instance_id = format!("{}-{}", display_name, unique_suffix());

        Self {
            instance_id,
            display_name,
            service_port,
            capabilities,
            peers: Arc::new(Mutex::new(HashMap::new())),
            running: Arc::new(AtomicBool::new(false)),
            broadcaster: None,
            listener: None,
        }
    }

    pub fn start(&mut self) -> io::Result<()> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Ok(());
        }

        let broadcast_socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))?;
        broadcast_socket.set_broadcast(true)?;

        let listener_socket =
            UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT))?;
        listener_socket.set_read_timeout(Some(Duration::from_millis(500)))?;

        let running = Arc::clone(&self.running);
        let packet = encode_packet(
            &self.instance_id,
            &self.display_name,
            self.service_port,
            &self.capabilities,
        );
        self.broadcaster = Some(thread::spawn(move || {
            while running.load(Ordering::SeqCst) {
                let _ = broadcast_socket.send_to(
                    packet.as_bytes(),
                    SocketAddrV4::new(Ipv4Addr::BROADCAST, DISCOVERY_PORT),
                );
                thread::sleep(BROADCAST_INTERVAL);
            }
        }));

        let running = Arc::clone(&self.running);
        let peers = Arc::clone(&self.peers);
        let instance_id = self.instance_id.clone();
        self.listener = Some(thread::spawn(move || {
            let mut buffer = [0u8; 1024];

            while running.load(Ordering::SeqCst) {
                match listener_socket.recv_from(&mut buffer) {
                    Ok((len, source)) => {
                        if let Some(packet) = parse_packet(&buffer[..len]) {
                            if packet.instance_id == instance_id {
                                continue;
                            }

                            let peer = Peer {
                                id: packet.instance_id.clone(),
                                display_name: packet.display_name,
                                addr: SocketAddr::new(source.ip(), packet.service_port),
                                capabilities: packet.capabilities,
                                last_seen_at: Instant::now(),
                            };

                            let mut peer_map = peers
                                .lock()
                                .expect("discovery peer lock should not be poisoned");
                            peer_map.insert(packet.instance_id, peer);
                            prune_peers(&mut peer_map);
                        }
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                        ) =>
                    {
                        let mut peer_map = peers
                            .lock()
                            .expect("discovery peer lock should not be poisoned");
                        prune_peers(&mut peer_map);
                    }
                    Err(_) => {}
                }
            }
        }));

        Ok(())
    }

    pub fn peers_snapshot(&self) -> Vec<Peer> {
        let mut peers = self
            .peers
            .lock()
            .expect("discovery peer lock should not be poisoned")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        peers.sort_by(|left, right| left.display_name.cmp(&right.display_name));
        peers
    }
}

impl Drop for DiscoveryEngine {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);

        if let Some(handle) = self.broadcaster.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.listener.take() {
            let _ = handle.join();
        }
    }
}

struct DiscoveryPacket {
    instance_id: String,
    display_name: String,
    service_port: u16,
    capabilities: Vec<String>,
}

fn encode_packet(
    instance_id: &str,
    display_name: &str,
    service_port: u16,
    capabilities: &[String],
) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}",
        DISCOVERY_MAGIC,
        DISCOVERY_VERSION,
        instance_id,
        display_name,
        service_port,
        capabilities.join(",")
    )
}

fn parse_packet(bytes: &[u8]) -> Option<DiscoveryPacket> {
    let payload = std::str::from_utf8(bytes).ok()?;
    let mut parts = payload.split('|');
    let magic = parts.next()?;
    let version = parts.next()?;
    if magic != DISCOVERY_MAGIC || version != DISCOVERY_VERSION {
        return None;
    }

    let instance_id = parts.next()?.to_owned();
    let display_name = parts.next()?.to_owned();
    let service_port = parts.next()?.parse::<u16>().ok()?;
    let capabilities = parts
        .next()
        .unwrap_or_default()
        .split(',')
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();

    Some(DiscoveryPacket {
        instance_id,
        display_name,
        service_port,
        capabilities,
    })
}

fn prune_peers(peers: &mut HashMap<String, Peer>) {
    peers.retain(|_, peer| peer.last_seen_at.elapsed() <= PEER_TTL);
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}
