#![feature(async_await)]

use std::{collections::HashMap, net::ToSocketAddrs};

use futures::{
    io::{BufReader, WriteHalf},
    stream::FuturesUnordered,
    channel::mpsc::{self, unbounded},
    SinkExt,
    select,
};

use async_std::{
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

#[derive(Debug)]
enum Event {
    NewPeer {
        name: String,
        stream: WriteHalf<TcpStream>,
    },
    Message {
        from: String,
        to: Vec<String>,
        msg: String,
    },
}

async fn server(addr: impl ToSocketAddrs) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;

    let (broker_sender, broker_receiver) = unbounded::<Event>();
    let broker = task::spawn(broker(broker_receiver));
    let mut incoming = listener.incoming().fuse();
    let mut tasks = FuturesUnordered::new();
    loop {
        let stream = select! {
            stream = incoming.next() => stream,
            res = tasks.next() => {
                if let Some(res) = res {
                    res?
                }
                continue;
            },
            complete => break,
        };

        let stream = stream.unwrap()?;
        println!("Accepting from: {}", stream.peer_addr()?);

        let handle = task::spawn(client(broker_sender.clone(), stream));
        tasks.push(handle);
    }
    broker.await?;
    Ok(())
}

async fn client(mut broker: Sender<Event>, stream: TcpStream) -> Result<()> {
    let (reader, writer) = stream.split();
    let reader = BufReader::new(reader);
    let mut lines = reader.lines();
    let name = match lines.next().await {
        None => Err("peer disconnected immediately")?,
        Some(line) => line?,
    };
    broker.send(Event::NewPeer { name: name.clone(), stream: writer }).await.unwrap();

    while let Some(line) = lines.next().await {
        let line = line?;
        let (dest, msg) = match line.find(':') {
            None => continue,
            Some(idx) => (&line[..idx], line[idx + 1 ..].trim()),
        };
        broker.send(Event::Message {
            from: name.clone(),
            to: dest.split(',').map(|name| name.trim().to_string()).collect(),
            msg: msg.trim().to_string(),
        }).await.unwrap();
    }
    Ok(())
}

async fn client_writer(
    mut receiver: Receiver<String>,
    mut writer: WriteHalf<TcpStream>,
) -> Result<()> {
    while let Some(msg) = receiver.next().await {
        writer.write_all(msg.as_bytes()).await?;
    }
    Ok(())
}

async fn broker(mut events: Receiver<Event>) -> Result<()> {
    let mut peers: HashMap<String, Sender<String>> = HashMap::new();
    let mut tasks = FuturesUnordered::new();

    loop {
        let event = select! {
            event = events.next() => event,
            res = tasks.next() => {
                if let Some(res) = res {
                    res?;
                }
                continue;
            }
            complete => break,
        };

        match event {
            Some(Event::Message { from, to, msg } )=> {
                for addr in to {
                    if let Some(peer) = peers.get_mut(&addr) {
                        peer.send(format!("from {}: {}\n", from, msg)).await?
                    }
                }
            }
            Some(Event::NewPeer { name, stream}) => {
                let (client_sender, client_receiver) = unbounded();
                peers.insert(name.clone(), client_sender);
                let handle = task::spawn(client_writer(client_receiver, stream));
                tasks.push(handle)
            }
            None => continue,
        }
    }
    Ok(())
}
