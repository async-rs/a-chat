#![feature(async_await)]

use std::{net::ToSocketAddrs, sync::Arc, collections::HashMap};

use futures::channel::mpsc;
use futures::SinkExt;

use async_std::{
    io::BufReader,
    prelude::*,
    task,
    net::{TcpListener, TcpStream},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
type Sender<T> = mpsc::UnboundedSender<T>;
type Receiver<T> = mpsc::UnboundedReceiver<T>;


fn main() -> Result<()> {
    task::block_on(server("127.0.0.1:8080"))
}

async fn server(addr: impl ToSocketAddrs) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;

    let (broker_sender, broker_receiver) = mpsc::unbounded();
    let broker = task::spawn(broker(broker_receiver));
    let mut incoming = listener.incoming();
    while let Some(stream) = incoming.next().await {
        let stream = stream?;
        println!("Accepting from: {}", stream.peer_addr()?);
        let _handle = task::spawn(client(broker_sender.clone(), stream));
    }
    broker.await?;
    Ok(())
}

async fn client(mut broker: Sender<Event>, stream: TcpStream) -> Result<()> {
    let stream = Arc::new(stream);
    let reader = BufReader::new(&*stream);
    let mut lines = reader.lines();

    let name = match lines.next().await {
        None => Err("peer disconnected immediately")?,
        Some(line) => line?,
    };
    broker.send(Event::NewPeer { name: name.clone(), stream: Arc::clone(&stream) }).await.unwrap();

    while let Some(line) = lines.next().await {
        let line = line?;
        let (dest, msg) = match line.find(':') {
            None => continue,
            Some(idx) => (&line[..idx], line[idx + 1 ..].trim()),
        };
        let dest: Vec<String> = dest.split(',').map(|name| name.trim().to_string()).collect();
        let msg: String = msg.trim().to_string();

        broker.send(Event::Message {
            from: name.clone(),
            to: dest,
            msg,
        }).await.unwrap();
    }
    Ok(())
}

async fn client_writer(
    mut messages: Receiver<String>,
    stream: Arc<TcpStream>,
) -> Result<()> {
    let mut stream = &*stream;
    while let Some(msg) = messages.next().await {
        stream.write_all(msg.as_bytes()).await?;
    }
    Ok(())
}

#[derive(Debug)]
enum Event {
    NewPeer {
        name: String,
        stream: Arc<TcpStream>,
    },
    Message {
        from: String,
        to: Vec<String>,
        msg: String,
    },
}

async fn broker(mut events: Receiver<Event>) -> Result<()> {
    let mut peers: HashMap<String, Sender<String>> = HashMap::new();

    while let Some(event) = events.next().await {
        match event {
            Event::Message { from, to, msg } => {
                for addr in to {
                    if let Some(peer) = peers.get_mut(&addr) {
                        peer.send(format!("from {}: {}\n", from, msg)).await?
                    }
                }
            }
            Event::NewPeer { name, stream} => {
                let (client_sender, client_receiver) = mpsc::unbounded();
                peers.insert(name.clone(), client_sender);
                let _handle = task::spawn(client_writer(client_receiver, stream));
            }
        }
    }
    Ok(())
}
