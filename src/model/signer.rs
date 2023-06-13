use crate::base::error::Error;
use crate::model::keystore::KeySigner;
use crate::model::keystr_model::{Event, EVENT_QUEUE};
use crate::model::status_messages::StatusMessages;

use nostr::nips::nip46::{Message, Request};
use nostr::prelude::{EventBuilder, Filter, Keys, Kind, NostrConnectURI, ToBech32, XOnlyPublicKey};
use nostr_sdk::prelude::{
    decrypt, Client, Options, RelayPoolNotification, RelayStatus, Response, Timestamp,
};

use crossbeam::channel;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::runtime::Handle;

/// Model for Signer
#[readonly::make]
pub(crate) struct Signer {
    app_id_keys: Keys,
    status: StatusMessages,
    #[readonly]
    connection: Option<Arc<SignerConnection>>,
    pub connect_uri_input: String,
}

/// Represents an active Nostr Connect connection
pub(crate) struct SignerConnection {
    // uri: NostrConnectURI,
    pub client_pubkey: XOnlyPublicKey,
    /// My client app ID, for the relays (not the one for signing)
    pub app_id_keys: Keys,
    status: StatusMessages,
    pub relay_str: String,
    relay_client: Client,
    key_signer: KeySigner,
    /// Holds pending requests (mostly Sign requests), and can handle them
    requests: Mutex<Vec<SignatureReqest>>,
}

#[derive(Clone)]
pub(crate) struct SignatureReqest {
    req: Message,
    sender_pubkey: XOnlyPublicKey,
}

/// Signer connection status: connected or not, or connection pending
pub(crate) enum ConnectionStatus {
    NotConnected,
    Connecting,
    Connected(Arc<SignerConnection>),
}

impl Signer {
    pub fn new(app_id: &Keys, status: StatusMessages) -> Self {
        Signer {
            app_id_keys: app_id.clone(),
            status,
            connection: None,
            connect_uri_input: String::new(),
        }
    }

    fn connect(&mut self, uri_str: &str, key_signer: &KeySigner) -> Result<(), Error> {
        if let ConnectionStatus::Connected(_) = self.get_connection_status() {
            return Err(Error::SignerAlreadyConnected);
        }

        let uri = &NostrConnectURI::from_str(uri_str)?;
        let connect_client_id_pubkey = uri.public_key.clone();
        let relay = &uri.relay_url;

        // Create relay client, but don't connect it yet
        let opts = Options::new().wait_for_send(true);
        let relay_client = Client::with_opts(&self.app_id_keys, opts);

        let connection = Arc::new(SignerConnection {
            // uri: uri.clone(),
            relay_str: relay.to_string(),
            relay_client,
            client_pubkey: connect_client_id_pubkey,
            status: self.status.clone(),
            app_id_keys: self.app_id_keys.clone(),
            key_signer: key_signer.clone(),
            requests: Mutex::new(Vec::new()),
        });

        let handle = tokio::runtime::Handle::current();
        // Connect in the background
        let _ = relay_connect_async(connection.clone(), handle)?;
        // Optimistic
        self.connection = Some(connection);
        Ok(())
    }

    fn disconnect(&mut self) -> Result<(), Error> {
        if let Some(conn) = &self.connection {
            let handle = tokio::runtime::Handle::current();
            let _res = relay_disconnect_blocking(conn.relay_client.clone(), handle)?;
        }
        self.connection = None;
        Ok(())
    }

    pub fn connect_action(&mut self, key_signer: KeySigner, status: &mut StatusMessages) {
        let uri_input = self.connect_uri_input.clone();
        match self.connect(&uri_input, &key_signer) {
            Err(e) => status.set_error(&format!("Could not connect to relay: {}", e.to_string())),
            Ok(_) => status.set(&format!("Signer connecting...")),
        }
    }

    pub fn disconnect_action(&mut self, status: &mut StatusMessages) {
        if let Some(_conn) = &self.connection {
            let _res_ignore = self.disconnect();
            status.set("Signer disconnected");
        }
        self.connection = None;
    }

    pub fn get_connection_status(&self) -> ConnectionStatus {
        match &self.connection {
            None => ConnectionStatus::NotConnected,
            Some(conn) => {
                let (connected, connecting) = match conn.get_connected_count() {
                    Err(_) => return ConnectionStatus::NotConnected,
                    Ok(tupl) => tupl,
                };
                if connected > 0 {
                    ConnectionStatus::Connected(conn.clone())
                } else if connecting > 0 {
                    ConnectionStatus::Connecting
                } else {
                    ConnectionStatus::NotConnected
                }
            }
        }
    }

    pub fn pending_process_first_action(&mut self, status: &mut StatusMessages) {
        if let Some(conn) = &self.connection {
            let first_desc = conn.get_first_request_description();
            conn.action_first_req_process();
            status.set(&format!("Processed request '{}'", first_desc));
        }
    }

    pub fn pending_ignore_first_action(&mut self, status: &mut StatusMessages) {
        if let Some(conn) = &self.connection {
            let first_desc = conn.get_first_request_description();
            conn.action_first_req_remove();
            status.set(&format!("Removed request '{}'", first_desc));
        }
    }

    /*
    fn get_relay_str(&self) -> String {
        match &self.connection {
            Some(conn) => conn.relay_str.clone(),
            None => "-".to_string(),
        }
    }

    fn get_client_npub(&self) -> String {
        if let Some(conn) = &self.connection {
            conn.client_pubkey.to_bech32().unwrap_or_default()
        } else {
            "-".to_string()
        }
    }
    */
}

impl SignerConnection {
    pub fn get_client_npub(&self) -> String {
        self.client_pubkey.to_bech32().unwrap_or_default()
    }

    pub fn add_request(&self, req: Message, sender_pubkey: XOnlyPublicKey) {
        self.requests
            .lock()
            .unwrap()
            .push(SignatureReqest { req, sender_pubkey });
    }

    pub fn get_pending_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    pub fn get_first_request_description(&self) -> String {
        let locked = self.requests.lock().unwrap();
        let first = locked.get(0);
        match first {
            None => "-".to_string(),
            Some(f) => f.description(),
        }
    }

    pub fn action_first_req_process(&self) {
        let mut locked = self.requests.lock().unwrap();
        let first = locked.first();
        if let Some(req) = first {
            if let Message::Request { id, .. } = &req.req {
                if let Ok(request) = &req.req.to_request() {
                    match request {
                        Request::SignEvent(unsigned_event) => {
                            let unsigned_id = unsigned_event.id;
                            if let Ok(signature) =
                                self.key_signer.sign(unsigned_id.as_bytes().to_vec())
                            {
                                let response_msg =
                                    Message::response(id.clone(), Response::SignEvent(signature));
                                let _ = send_message_blocking(
                                    &self.relay_client,
                                    &response_msg,
                                    &req.sender_pubkey,
                                    tokio::runtime::Handle::current(),
                                );
                            }
                        }
                        // ignore other requests
                        _ => {}
                    }
                }
            }
        }
        let _ = locked.remove(0);
    }

    /// Remove the (first) pending request
    pub fn action_first_req_remove(&self) {
        let _ = self.requests.lock().unwrap().remove(0);
    }

    /// Get number of relays that are Connected / Connecting
    pub async fn get_connected_count_bg(relay_client: &Client) -> (u32, u32) {
        let relays = relay_client.relays().await;
        let (mut cnt_cncted, mut cnt_cncting) = (0, 0);
        for (_k, r) in relays {
            match r.status().await {
                RelayStatus::Connected => cnt_cncted = cnt_cncted + 1,
                RelayStatus::Connecting => cnt_cncting = cnt_cncting + 1,
                _ => (),
            }
        }
        (cnt_cncted, cnt_cncting)
    }

    /// Get number of relays that are Connected / Connecting, blocking version
    pub fn get_connected_count(&self) -> Result<(u32, u32), Error> {
        let (tx, rx) = channel::bounded(1);
        let relay_client_clone = self.relay_client.clone();
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            let count = Self::get_connected_count_bg(&relay_client_clone).await;
            let _ = tx.send(count);
        });
        Ok(rx.recv()?)
    }
}

const PREVIEW_CONTENT_LEN: usize = 100;

fn shortened_text(text: &str, max_len: usize) -> String {
    if text.len() < max_len {
        text.to_string()
    } else {
        format!("{}..", text[0..max_len].to_string())
    }
}

impl SignatureReqest {
    pub fn description(&self) -> String {
        match self.req.to_request() {
            Err(_) => "(not request, no action needed)".to_string(),
            Ok(req) => match req {
                Request::SignEvent(unsigned_event) => {
                    format!(
                        "Signature requested for message: '{}'",
                        shortened_text(&unsigned_event.content, PREVIEW_CONTENT_LEN)
                    )
                }
                _ => format!("({}, no action needed)", req.method()),
            },
        }
    }
}

async fn send_message(
    relay_client: &Client,
    msg: &Message,
    receiver_pubkey: &XOnlyPublicKey,
) -> Result<(), Error> {
    let keys = relay_client.keys();
    let event =
        EventBuilder::nostr_connect(&keys, *receiver_pubkey, msg.clone())?.to_event(&keys)?;
    relay_client.send_event(event).await?;
    println!("DEBUG: Message sent, {:?}", msg);
    Ok(())
}

fn send_message_blocking(
    relay_client: &Client,
    msg: &Message,
    receiver_pubkey: &XOnlyPublicKey,
    handle: Handle,
) -> Result<(), Error> {
    let (tx, rx) = channel::bounded(1);
    let relay_client_clone = relay_client.clone();
    let msg_clone = msg.clone();
    let receiver_pubkey_clone = receiver_pubkey.clone();
    handle.spawn(async move {
        let res = send_message(&relay_client_clone, &msg_clone, &receiver_pubkey_clone).await;
        let _ = tx.send(res);
    });
    let res = rx.recv()?;
    res
}

async fn relay_connect(
    connection: Arc<SignerConnection>,
    connect_id_keys: &Keys,
) -> Result<(), Error> {
    connection
        .relay_client
        .add_relay(&connection.relay_str, None)
        .await?;
    // TODO: SDK does not give an error here
    connection.relay_client.connect().await;

    let _res = start_handler_loop(connection.clone(), tokio::runtime::Handle::current())?;

    // Send connect ACK
    let msg = Message::request(Request::Connect(connect_id_keys.public_key()));
    let _ = send_message(&connection.relay_client, &msg, &connection.client_pubkey).await?;

    EVENT_QUEUE.push(Event::SignerConnected)?;
    connection.status.set(&format!(
        "Signer connected (relay: {}, client npub: {})",
        connection.relay_str,
        connection.client_pubkey.to_bech32().unwrap(),
    ));

    Ok(())
}

async fn relay_disconnect(relay_client: Client) -> Result<(), Error> {
    let _res = relay_client.disconnect().await?;
    Ok(())
}

/*
fn relay_connect_blocking(connection: Arc<SignerConnection>, handle: Handle) -> Result<(), Error> {
    let (tx, rx) = channel::bounded(1);
    let connect_id_keys_clone = connection.app_id_keys.clone();
    let connection_clone = connection.clone();
    handle.spawn(async move {
        let conn_res = relay_connect(connection_clone, &connect_id_keys_clone).await;
        let _ = tx.send(conn_res);
    });
    let _ = rx.recv()?;
    Ok(())
}
*/

/// Do connect in the background
fn relay_connect_async(connection: Arc<SignerConnection>, handle: Handle) -> Result<(), Error> {
    let connect_id_keys_clone = connection.app_id_keys.clone();
    let connection_clone = connection.clone();
    handle.spawn(async move {
        let _ = relay_connect(connection_clone, &connect_id_keys_clone).await;
    });
    Ok(())
}

fn relay_disconnect_blocking(relay_client: Client, handle: Handle) -> Result<(), Error> {
    let (tx, rx) = channel::bounded(1);
    let relay_client_clone = relay_client.clone();
    handle.spawn(async move {
        let disconn_res = relay_disconnect(relay_client_clone).await;
        let _ = tx.send(disconn_res);
    });
    rx.recv()?
}

fn message_method(msg: &Message) -> String {
    match &msg {
        Message::Request { method, .. } => format!("request {method}"),
        Message::Response { .. } => "response".to_string(),
    }
}

/// Start event handling loop in the background, asynchrnous, fire-and-forget
// TODO: Close loop on disconnect!
fn start_handler_loop(connection: Arc<SignerConnection>, handle: Handle) -> Result<(), Error> {
    // let (tx, rx) = channel::bounded(1);
    let connection_clone = connection.clone();
    handle.spawn(async move {
        let _res = wait_and_handle_messages(connection_clone).await;
        // let _ = tx.send(res);
    });
    // rx.recv()?
    Ok(())
}

async fn wait_and_handle_messages(connection: Arc<SignerConnection>) -> Result<(), Error> {
    let relay_client = &connection.relay_client;
    let keys = relay_client.keys();

    relay_client
        .subscribe(vec![Filter::new()
            .pubkey(keys.public_key())
            .kind(Kind::NostrConnect)
            .since(Timestamp::now() - Duration::from_secs(10))])
        .await;
    println!("DEBUG: Subscribed to relay events ...");
    println!("DEBUG: Waiting for messages ...");

    loop {
        let mut notifications = relay_client.notifications();
        while let Ok(notification) = notifications.recv().await {
            if let RelayPoolNotification::Event(_url, event) = notification {
                if event.kind == Kind::NostrConnect {
                    match decrypt(&keys.secret_key()?, &event.pubkey, &event.content) {
                        Ok(msg) => {
                            let msg = Message::from_json(msg)?;
                            let _ = handle_request(connection.clone(), &msg, &event.pubkey).await?;
                        }
                        Err(e) => eprintln!("DEBUG: Impossible to decrypt NIP46 message: {e}"),
                    }
                }
            }
        }
    }
    // relay_client.unsubscribe().await;
}

fn response_for_message(req_id: &String, req: &Request, key_signer: &KeySigner) -> Option<Message> {
    match req {
        Request::Describe => {
            println!("DEBUG: Describe received");
            let values = ["describe", "get_public_key", "sign_event"]
                .to_vec()
                .iter()
                .map(|s| s.to_string())
                .collect();
            Some(Message::response(
                req_id.to_string(),
                Response::Describe(values),
            ))
        }
        Request::GetPublicKey => {
            // Return the signer pubkey
            println!("DEBUG: GetPublicKey received");
            Some(Message::response(
                req_id.clone(),
                Response::GetPublicKey(key_signer.get_public_key()),
            ))
        }
        Request::SignEvent(_) | _ => None,
    }
}

async fn handle_request(
    connection: Arc<SignerConnection>,
    msg: &Message,
    sender_pubkey: &XOnlyPublicKey,
) -> Result<(), Error> {
    println!("DEBUG: New message received {}", message_method(msg));

    if let Message::Request { id, .. } = msg {
        if let Ok(req) = &msg.to_request() {
            let key_signer = &connection.key_signer;
            let response_message = response_for_message(id, req, key_signer);
            match response_message {
                Some(m) => {
                    // We return a response message right away
                    let relay_client = &connection.relay_client;
                    let _ = send_message(relay_client, &m, sender_pubkey).await?;
                }
                None => {
                    // Cannot return a response message right away, other handling needed
                    match req {
                        Request::SignEvent(_) => {
                            // This request needs user processing, store it, notify it
                            connection.add_request(msg.clone(), sender_pubkey.clone());
                            EVENT_QUEUE.push(Event::SignerNewRequest)?;
                            connection.status.set("New Signing request received");
                        }
                        _ => {
                            println!("DEBUG: Unhandled Request {:?}", msg.to_request());
                        }
                    }
                }
            }
        } else {
            println!("DEBUG: Could not extract Request, ignoring");
        }
    } else {
        println!("DEBUG: Not a Request, ignoring");
    }
    Ok(())
}
