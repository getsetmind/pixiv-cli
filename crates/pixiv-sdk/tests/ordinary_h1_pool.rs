use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::{JoinHandle, JoinSet};

const BOUND: Duration = Duration::from_secs(4);
const TTL: Duration = Duration::from_millis(350);

#[derive(Debug)]
enum Event {
    Request(usize, String),
    Closed(usize),
}

struct Peer {
    url: String,
    releases: Arc<Semaphore>,
    events: mpsc::UnboundedReceiver<Event>,
    closed: BTreeSet<usize>,
    server: JoinHandle<()>,
}

impl Peer {
    async fn new() -> Self {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let releases = Arc::new(Semaphore::new(0));
        let (tx, events) = mpsc::unbounded_channel();
        let gates = Arc::clone(&releases);
        let server = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            let mut id = 0;
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (socket, _) = accepted.unwrap();
                        id += 1;
                        let tx = tx.clone();
                        let gates = Arc::clone(&gates);
                        let connection = id;
                        connections.spawn(async move {
                            let _ = tokio::time::timeout(BOUND, serve(socket, connection, &tx, gates)).await;
                            let _ = tx.send(Event::Closed(connection));
                        });
                    }
                    Some(result) = connections.join_next(), if !connections.is_empty() => {
                        result.unwrap();
                    }
                }
            }
        });
        Self {
            url,
            releases,
            events,
            closed: BTreeSet::new(),
            server,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.url)
    }

    fn release(&self, count: usize) {
        self.releases.add_permits(count);
    }

    async fn request_received(&mut self, expected: &str) -> usize {
        tokio::time::timeout(BOUND, async {
            loop {
                match self.events.recv().await.unwrap() {
                    Event::Request(id, path) if path == expected => return id,
                    Event::Request(_, _) => {}
                    Event::Closed(id) => {
                        self.closed.insert(id);
                    }
                }
            }
        })
        .await
        .unwrap()
    }

    async fn closed(&mut self, id: usize) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while !self.closed.contains(&id) {
                match self.events.recv().await.unwrap() {
                    Event::Closed(connection) => {
                        self.closed.insert(connection);
                    }
                    Event::Request(_, _) => {}
                }
            }
        })
        .await
        .unwrap();
    }

    async fn remains_open(&mut self, id: usize, duration: Duration) {
        let deadline = tokio::time::Instant::now() + duration;
        while let Ok(Some(event)) = tokio::time::timeout_at(deadline, self.events.recv()).await {
            if let Event::Closed(connection) = event {
                self.closed.insert(connection);
            }
            assert!(
                !self.closed.contains(&id),
                "connection {id} closed before its idle deadline"
            );
        }
        assert!(!self.closed.contains(&id));
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn serve(
    mut socket: TcpStream,
    id: usize,
    tx: &mpsc::UnboundedSender<Event>,
    releases: Arc<Semaphore>,
) -> std::io::Result<()> {
    loop {
        let mut request = Vec::new();
        loop {
            let byte = socket.read_u8().await?;
            request.push(byte);
            if request.ends_with(b"\r\n\r\n") {
                break;
            }
            assert!(request.len() < 16_384);
        }
        let request = String::from_utf8(request).unwrap();
        let path = request.split_whitespace().nth(1).unwrap().to_owned();
        tx.send(Event::Request(id, path.clone())).unwrap();
        if path.starts_with("/headers") {
            releases.acquire().await.unwrap().forget();
        }
        socket
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: 8\r\nX-Fixture-Connection: {id}\r\n\r\n"
                )
                .as_bytes(),
            )
            .await?;
        socket.write_all(b"abcd").await?;
        if path.starts_with("/body") {
            releases.acquire().await.unwrap().forget();
        }
        socket.write_all(b"efgh").await?;
        socket.flush().await?;
    }
}

fn client(ttl: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .http1_only()
        .pool_idle_retention(true)
        .pool_idle_timeout(ttl)
        .pool_max_idle_per_host(2)
        .pool_max_idle_connections(100)
        .build()
        .unwrap()
}

async fn response(client: &reqwest::Client, peer: &Peer, path: &str) -> reqwest::Response {
    tokio::time::timeout(BOUND, client.get(peer.url(path)).send())
        .await
        .unwrap()
        .unwrap()
}

fn connection(response: &reqwest::Response) -> usize {
    response.headers()["x-fixture-connection"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap()
}

async fn consume(response: reqwest::Response) {
    assert_eq!(
        tokio::time::timeout(BOUND, response.bytes())
            .await
            .unwrap()
            .unwrap()
            .as_ref(),
        b"abcdefgh"
    );
}

async fn roundtrip(client: &reqwest::Client, peer: &Peer, path: &str) -> usize {
    let response = response(client, peer, path).await;
    let id = connection(&response);
    consume(response).await;
    id
}

#[tokio::test]
async fn idle_close_is_repeatable_and_an_independent_pool_remains_reusable() {
    let mut peer = Peer::new().await;
    let a = client(Duration::from_secs(2));
    let b = client(Duration::from_secs(2));
    let aid = roundtrip(&a, &peer, "/a").await;
    assert_eq!(roundtrip(&a, &peer, "/a2").await, aid);
    let bid = roundtrip(&b, &peer, "/b").await;
    a.close_idle_connections();
    a.close_idle_connections();
    peer.closed(aid).await;
    assert_eq!(roundtrip(&b, &peer, "/b2").await, bid);
    assert_ne!(roundtrip(&a, &peer, "/a3").await, aid);
    a.close_idle_connections();
    b.close_idle_connections();
}

#[tokio::test]
async fn close_during_active_body_preserves_bytes_then_physically_retires_the_socket() {
    let mut peer = Peer::new().await;
    let client = client(Duration::from_secs(2));
    let response = response(&client, &peer, "/body").await;
    let id = connection(&response);
    client.close_idle_connections();
    client.close_idle_connections();
    peer.remains_open(id, Duration::from_millis(80)).await;
    peer.release(1);
    consume(response).await;
    peer.closed(id).await;
}

#[tokio::test]
async fn close_during_active_headers_preserves_the_request_then_retires_it() {
    let mut peer = Peer::new().await;
    let client = client(Duration::from_secs(2));
    let request = tokio::spawn({
        let client = client.clone();
        let url = peer.url("/headers");
        async move { client.get(url).send().await.unwrap() }
    });
    let id = peer.request_received("/headers").await;
    client.close_idle_connections();
    peer.remains_open(id, Duration::from_millis(80)).await;
    peer.release(1);
    consume(tokio::time::timeout(BOUND, request).await.unwrap().unwrap()).await;
    peer.closed(id).await;
}

#[tokio::test]
async fn new_checkout_resets_future_idle_retirement_once() {
    let mut peer = Peer::new().await;
    let client = client(Duration::from_secs(2));
    let active = response(&client, &peer, "/body").await;
    let active_id = connection(&active);
    client.close_idle_connections();
    let second_id = roundtrip(&client, &peer, "/reset").await;
    assert_ne!(active_id, second_id);
    peer.release(1);
    consume(active).await;
    peer.remains_open(active_id, Duration::from_millis(80))
        .await;
    assert_eq!(roundtrip(&client, &peer, "/reuse").await, active_id);
    client.close_idle_connections();
    peer.closed(active_id).await;
    peer.closed(second_id).await;
}

#[tokio::test]
async fn owner_drop_retains_an_idle_socket_until_its_actual_idle_deadline() {
    let mut peer = Peer::new().await;
    let client = client(TTL);
    let id = roundtrip(&client, &peer, "/idle").await;
    drop(client);
    peer.remains_open(id, TTL / 2).await;
    peer.closed(id).await;
}

#[tokio::test]
async fn owner_drop_keeps_an_active_body_and_starts_ttl_when_it_really_returns_idle() {
    let mut peer = Peer::new().await;
    let client = client(TTL);
    let response = response(&client, &peer, "/body").await;
    let id = connection(&response);
    drop(client);
    peer.remains_open(id, TTL + Duration::from_millis(80)).await;
    peer.release(1);
    consume(response).await;
    peer.remains_open(id, TTL / 2).await;
    peer.closed(id).await;
}

#[tokio::test]
async fn an_old_idle_entry_expiry_cannot_retire_a_checked_out_or_reinserted_connection() {
    let mut peer = Peer::new().await;
    let client = client(TTL);
    let id = roundtrip(&client, &peer, "/first").await;
    let response = response(&client, &peer, "/body").await;
    assert_eq!(connection(&response), id);
    drop(client);
    peer.remains_open(id, TTL + Duration::from_millis(80)).await;
    peer.release(1);
    consume(response).await;
    peer.remains_open(id, TTL / 2).await;
    peer.closed(id).await;
}

#[tokio::test]
async fn each_idle_connection_has_its_own_deadline_after_owner_drop() {
    let mut peer = Peer::new().await;
    let client = client(TTL);
    let a = response(&client, &peer, "/body/a").await;
    let b = response(&client, &peer, "/body/b").await;
    let aid = connection(&a);
    let bid = connection(&b);
    peer.release(1);
    consume(a).await;
    tokio::time::sleep(TTL * 3 / 5).await;
    peer.release(1);
    consume(b).await;
    drop(client);
    peer.closed(aid).await;
    assert!(!peer.closed.contains(&bid));
    peer.remains_open(bid, TTL / 5).await;
    peer.closed(bid).await;
}

#[tokio::test]
async fn dropping_an_unfinished_body_retires_instead_of_pooling_it() {
    let mut peer = Peer::new().await;
    let client = client(Duration::from_secs(2));
    let response = response(&client, &peer, "/body").await;
    let id = connection(&response);
    drop(response);
    client.close_idle_connections();
    peer.release(1);
    peer.closed(id).await;
    assert_ne!(roundtrip(&client, &peer, "/next").await, id);
    client.close_idle_connections();
}

#[tokio::test]
async fn global_idle_limit_retires_the_oldest_host_without_interrupting_another_host() {
    let mut a = Peer::new().await;
    let mut b = Peer::new().await;
    let client = reqwest::Client::builder()
        .no_proxy()
        .http1_only()
        .pool_idle_retention(true)
        .pool_idle_timeout(Duration::from_secs(2))
        .pool_max_idle_per_host(2)
        .pool_max_idle_connections(1)
        .build()
        .unwrap();
    let aid = roundtrip(&client, &a, "/a").await;
    let bid = roundtrip(&client, &b, "/b").await;
    a.closed(aid).await;
    assert_eq!(roundtrip(&client, &b, "/b-reuse").await, bid);
    client.close_idle_connections();
    b.closed(bid).await;
}

#[tokio::test]
async fn host_idle_limit_keeps_two_returned_sockets_and_retires_the_third() {
    let mut peer = Peer::new().await;
    let client = client(Duration::from_secs(2));
    let a = response(&client, &peer, "/body/a").await;
    let b = response(&client, &peer, "/body/b").await;
    let c = response(&client, &peer, "/body/c").await;
    let aid = connection(&a);
    let bid = connection(&b);
    let cid = connection(&c);
    peer.release(3);
    consume(a).await;
    consume(b).await;
    consume(c).await;
    tokio::time::timeout(BOUND, async {
        while peer.closed.is_empty() {
            if let Event::Closed(id) = peer.events.recv().await.unwrap() {
                peer.closed.insert(id);
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(peer.closed.len(), 1);
    assert!(peer.closed.is_subset(&BTreeSet::from([aid, bid, cid])));
    let remaining = BTreeSet::from([aid, bid, cid])
        .difference(&peer.closed)
        .copied()
        .collect::<BTreeSet<_>>();
    let reused = roundtrip(&client, &peer, "/reuse").await;
    assert!(remaining.contains(&reused));
    client.close_idle_connections();
    for id in remaining {
        peer.closed(id).await;
    }
}

#[tokio::test]
async fn explicit_idle_close_releases_retained_entries_without_waiting_for_ttl() {
    let mut peer = Peer::new().await;
    let client = client(Duration::from_secs(3));
    let id = roundtrip(&client, &peer, "/idle").await;
    let before = Instant::now();
    client.close_idle_connections();
    drop(client);
    peer.closed(id).await;
    assert!(before.elapsed() < Duration::from_secs(1));
}

struct GatedResolver {
    address: std::net::SocketAddr,
    calls: AtomicUsize,
    started: mpsc::UnboundedSender<usize>,
    releases: Arc<Semaphore>,
}

impl reqwest::dns::Resolve for GatedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        assert_eq!(name.as_str(), "fixture.invalid");
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let address = self.address;
        let started = self.started.clone();
        let releases = Arc::clone(&self.releases);
        Box::pin(async move {
            started.send(call).unwrap();
            if call != 0 {
                releases.acquire().await.unwrap().forget();
            }
            let addresses: reqwest::dns::Addrs = Box::new(std::iter::once(address));
            Ok(addresses)
        })
    }
}

#[tokio::test]
async fn close_preserves_a_parked_waiter_without_letting_its_old_checkout_clear_the_flag() {
    let mut peer = Peer::new().await;
    let address = peer
        .url
        .trim_start_matches("http://")
        .parse::<std::net::SocketAddr>()
        .unwrap();
    let (started, mut resolutions) = mpsc::unbounded_channel();
    let dial_release = Arc::new(Semaphore::new(0));
    let resolver = Arc::new(GatedResolver {
        address,
        calls: AtomicUsize::new(0),
        started,
        releases: Arc::clone(&dial_release),
    });
    let client = reqwest::Client::builder()
        .no_proxy()
        .http1_only()
        .dns_resolver(resolver)
        .pool_idle_retention(true)
        .pool_idle_timeout(Duration::from_secs(3))
        .pool_max_idle_per_host(2)
        .pool_max_idle_connections(100)
        .build()
        .unwrap();
    let base = format!("http://fixture.invalid:{}", address.port());
    let active = tokio::time::timeout(BOUND, client.get(format!("{base}/body")).send())
        .await
        .unwrap()
        .unwrap();
    let id = connection(&active);
    assert_eq!(resolutions.recv().await.unwrap(), 0);
    let waiting = tokio::spawn({
        let client = client.clone();
        let url = format!("{base}/waiter");
        async move { client.get(url).send().await.unwrap() }
    });
    assert_eq!(
        tokio::time::timeout(BOUND, resolutions.recv())
            .await
            .unwrap()
            .unwrap(),
        1
    );
    client.close_idle_connections();
    peer.release(1);
    consume(active).await;
    let response = tokio::time::timeout(BOUND, waiting).await.unwrap().unwrap();
    assert_eq!(connection(&response), id);
    consume(response).await;
    peer.closed(id).await;
    dial_release.add_permits(1);
    peer.closed(id + 1).await;
    client.close_idle_connections();
}
