use std::collections::{HashMap, VecDeque};

use async_std::channel::{Receiver, Sender};
use atm0s_sdn::NodeId;
use futures::{select, FutureExt};

use super::vsocket::VirtualUdpSocket;

#[derive(Debug)]
pub enum OutEvent {
    Bind(u16),
    Pkt(NetworkPkt),
    Unbind(u16),
}

#[derive(Debug)]
pub struct NetworkPkt {
    pub local_port: u16,
    pub remote: NodeId,
    pub remote_port: u16,
    pub data: Vec<u8>,
    pub meta: u8,
}

pub struct VirtualNetwork {
    node_id: NodeId,
    in_rx: Receiver<NetworkPkt>,
    out_tx: Sender<OutEvent>,
    close_socket_tx: Sender<u16>,
    close_socket_rx: Receiver<u16>,
    sockets: HashMap<u16, Sender<NetworkPkt>>,
    ports: VecDeque<u16>,
}

impl VirtualNetwork {
    pub fn new(node_id: NodeId) -> (Self, Sender<NetworkPkt>, Receiver<OutEvent>) {
        let (in_tx, in_rx) = async_std::channel::bounded(1000);
        let (out_tx, out_rx) = async_std::channel::bounded(1000);
        let (close_socket_tx, close_socket_rx) = async_std::channel::unbounded();

        (
            Self {
                node_id,
                in_rx,
                out_tx,
                close_socket_rx,
                close_socket_tx,
                sockets: HashMap::new(),
                ports: (0..60000).collect(),
            },
            in_tx,
            out_rx,
        )
    }

    pub async fn udp_socket(&mut self, port: u16) -> Option<VirtualUdpSocket> {
        //remove port from ports
        let port = if port > 0 {
            let index = self
                .ports
                .iter()
                .position(|&x| x == port)
                .expect("Should have port");
            self.ports.swap_remove_back(index);
            port
        } else {
            self.ports.pop_front()?
        };
        self.out_tx
            .send(OutEvent::Bind(port))
            .await
            .expect("Should send bind");
        let (tx, rx) = async_std::channel::bounded(1000);
        self.sockets.insert(port, tx);
        Some(VirtualUdpSocket::new(
            self.node_id,
            port,
            self.out_tx.clone(),
            rx,
            self.close_socket_tx.clone(),
        ))
    }

    pub async fn recv(&mut self) -> Option<()> {
        select! {
            port = self.close_socket_rx.recv().fuse() => {
                let port = port.expect("Should have port");
                self.ports.push_back(port);
                self.sockets.remove(&port);
                self.out_tx.send(OutEvent::Unbind(port)).await.expect("Should send unbind");
                Some(())
            }
            pkt = self.in_rx.recv().fuse() => {
                let pkt = pkt.ok()?;
                let src = pkt.local_port;
                if let Some(socket_tx) = self.sockets.get(&src) {
                    if let Err(e) = socket_tx.try_send(pkt) {
                        log::error!("Send to socket {} error {:?}", src, e);
                    }
                } else {
                    log::warn!("No socket for port {}", src);
                }
                Some(())
            }
        }
    }
}
