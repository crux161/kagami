use std::collections::HashMap;
use std::env;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use mdns_sd::{ResolvedService, ScopedIp, ServiceDaemon, ServiceEvent, ServiceInfo};

pub const KAGAMI_SERVICE_TYPE: &str = "_kagami._udp.local.";
const TXT_CAPABILITIES: &str = "capabilities";
const TXT_DISPLAY_NAME: &str = "display-name";
const TXT_INSTANCE_ID: &str = "instance-id";
const TXT_PORT: &str = "port";

#[derive(Clone, Debug)]
pub struct Peer {
    pub id: String,
    pub display_name: String,
    pub host_name: String,
    pub addr: SocketAddr,
    pub capabilities: Vec<String>,
}

impl Peer {
    pub fn label(&self) -> String {
        let capabilities = if self.capabilities.is_empty() {
            "No capabilities".to_owned()
        } else {
            self.capabilities.join(", ")
        };

        format!(
            "{} ({}) · {} · {}",
            self.display_name, self.host_name, self.addr, capabilities
        )
    }
}

pub struct DiscoveryManager {
    daemon: ServiceDaemon,
    local_fullname: String,
    peers: Arc<Mutex<HashMap<String, Peer>>>,
    browser: Option<JoinHandle<()>>,
}

impl DiscoveryManager {
    pub fn new(
        service_port: u16,
        instance_id: String,
        capabilities: Vec<String>,
    ) -> Result<Self, String> {
        let daemon = ServiceDaemon::new()
            .map_err(|error| format!("failed to start mDNS discovery daemon: {error}"))?;
        let display_name = local_display_name();
        let instance_name = format!("{display_name}-{instance_id}");
        let host_name = format!("{}.local.", sanitize_dns_label(&local_host_label()));
        let addresses = local_addresses();
        let local_listener_addrs = addresses
            .iter()
            .copied()
            .map(|ip| SocketAddr::new(ip, service_port))
            .collect::<Vec<_>>();
        let properties = [
            (TXT_CAPABILITIES, capabilities.join(",")),
            (TXT_DISPLAY_NAME, display_name.clone()),
            (TXT_INSTANCE_ID, instance_id.clone()),
            (TXT_PORT, service_port.to_string()),
        ];

        let service_info = ServiceInfo::new(
            KAGAMI_SERVICE_TYPE,
            &instance_name,
            &host_name,
            addresses.as_slice(),
            service_port,
            &properties[..],
        )
        .map_err(|error| format!("failed to build mDNS service record: {error}"))?
        .enable_addr_auto();
        let local_fullname = service_info.get_fullname().to_owned();

        daemon
            .register(service_info)
            .map_err(|error| format!("failed to register mDNS service: {error}"))?;

        let receiver = daemon
            .browse(KAGAMI_SERVICE_TYPE)
            .map_err(|error| format!("failed to browse mDNS services: {error}"))?;

        let peers = Arc::new(Mutex::new(HashMap::<String, Peer>::new()));
        let peer_store = Arc::clone(&peers);
        let local_fullname_for_thread = local_fullname.clone();
        let local_instance_id_for_thread = instance_id;

        let browser = thread::spawn(move || {
            while let Ok(event) = receiver.recv() {
                match event {
                    ServiceEvent::ServiceResolved(service) => {
                        if service.get_fullname() == local_fullname_for_thread
                            || resolved_instance_id(&service).is_some_and(|instance_id| {
                                instance_id == local_instance_id_for_thread
                            })
                        {
                            continue;
                        }

                        if let Some(peer) = peer_from_resolved(&service) {
                            if local_listener_addrs.contains(&peer.addr) {
                                continue;
                            }

                            let mut peers = peer_store
                                .lock()
                                .expect("discovery peer store should not be poisoned");
                            peers.insert(peer.id.clone(), peer);
                        }
                    }
                    ServiceEvent::ServiceRemoved(_, fullname) => {
                        let mut peers = peer_store
                            .lock()
                            .expect("discovery peer store should not be poisoned");
                        peers.remove(&fullname);
                    }
                    _ => {}
                }
            }
        });

        Ok(Self {
            daemon,
            local_fullname,
            peers,
            browser: Some(browser),
        })
    }

    pub fn peers_snapshot(&self) -> Vec<Peer> {
        let mut peers = self
            .peers
            .lock()
            .expect("discovery peer store should not be poisoned")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        peers.sort_by(|left, right| left.display_name.cmp(&right.display_name));
        peers
    }
}

impl Drop for DiscoveryManager {
    fn drop(&mut self) {
        let _ = self.daemon.stop_browse(KAGAMI_SERVICE_TYPE);
        let _ = self.daemon.unregister(&self.local_fullname);
        let _ = self.daemon.shutdown();

        if let Some(browser) = self.browser.take() {
            let _ = browser.join();
        }
    }
}

fn peer_from_resolved(service: &ResolvedService) -> Option<Peer> {
    let id = resolved_instance_id(service).unwrap_or_else(|| service.get_fullname().to_owned());
    let port = service
        .get_properties()
        .get_property_val_str(TXT_PORT)
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(service.get_port());
    let addr = service
        .get_addresses()
        .iter()
        .find_map(scoped_ip_to_ip)
        .map(|ip| SocketAddr::new(ip, port))?;
    let capabilities = service
        .get_properties()
        .get_property_val_str(TXT_CAPABILITIES)
        .unwrap_or_default()
        .split(',')
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();

    Some(Peer {
        id,
        display_name: service
            .get_properties()
            .get_property_val_str(TXT_DISPLAY_NAME)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| service.get_fullname().split('.').next().unwrap_or_default())
            .to_owned(),
        host_name: service.get_hostname().trim_end_matches('.').to_owned(),
        addr,
        capabilities,
    })
}

fn resolved_instance_id(service: &ResolvedService) -> Option<String> {
    service
        .get_properties()
        .get_property_val_str(TXT_INSTANCE_ID)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn scoped_ip_to_ip(address: &ScopedIp) -> Option<IpAddr> {
    match address {
        ScopedIp::V4(address) => Some(IpAddr::V4(*address.addr())),
        ScopedIp::V6(address) => Some(IpAddr::V6(*address.addr())),
        _ => None,
    }
}

fn local_addresses() -> Vec<IpAddr> {
    let mut addresses = Vec::new();

    if let Ok(socket) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) {
        let _ = socket.connect((Ipv4Addr::new(224, 0, 0, 251), 5353));
        if let Ok(local_addr) = socket.local_addr() {
            let ip = local_addr.ip();
            if !ip.is_unspecified() && !ip.is_loopback() {
                addresses.push(ip);
            }
        }
    }

    if addresses.is_empty() {
        addresses.push(IpAddr::V4(Ipv4Addr::LOCALHOST));
    }

    addresses
}

fn local_display_name() -> String {
    env::var("HOSTNAME")
        .or_else(|_| env::var("COMPUTERNAME"))
        .or_else(|_| env::var("USER"))
        .unwrap_or_else(|_| "Kagami".to_owned())
}

fn local_host_label() -> String {
    env::var("HOSTNAME")
        .or_else(|_| env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "kagami".to_owned())
}

fn sanitize_dns_label(value: &str) -> String {
    let mut label = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();

    label.truncate(63);
    label = label.trim_matches('-').to_owned();
    if label.is_empty() {
        "kagami".to_owned()
    } else {
        label
    }
}
