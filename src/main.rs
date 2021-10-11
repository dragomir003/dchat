use std::collections::HashMap;
use std::sync::Arc;

use async_std::channel::{unbounded, Receiver, Sender};
use async_std::task::JoinHandle;
use async_std::{
    io::{prelude::BufReadExt, BufReader},
    net::{TcpListener, TcpStream},
    task,
};
use futures::{AsyncWriteExt, StreamExt};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug)]
enum Event {
    Login {
        name: String,
        stream: Arc<TcpStream>,
    },
    Message {
        from: String,
        to: Vec<String>,
        msg: String,
    },
    Logout {
        name: String,
    },
    Empty,
}

fn main() -> Result<()> {
    let fut = start_server();

    task::block_on(fut)
}

async fn start_server() -> Result<()> {
    let listener = TcpListener::bind("192.168.0.52:12123").await?;

    let mut incoming = listener.incoming();

    let (sender, receiver) = unbounded::<Event>();

    task::spawn(handle_events(receiver));

    while let Some(stream) = incoming.next().await {
        let mut stream = stream?;

        println!("Connection from: {}", stream.peer_addr()?);
        stream.write_all(b"Connected\n").await?;

        task::spawn(handle_connection(stream, sender.clone()));
    }

    Ok(())
}

async fn handle_connection(stream: TcpStream, event_sender: Sender<Event>) -> Result<()> {
    let stream = Arc::new(stream);
    let mut lines = BufReader::new(&*stream).lines();

    let mut name: String = String::default();

    while let Some(line) = lines.next().await {
        let event = to_event(line?, Arc::clone(&stream));

        match &event {
            Event::Login {
                name: name_,
                stream: _stream,
            } => name = name_.clone(),
            _ => {}
        }

        event_sender.send(event).await?;
    }

    event_sender.send(Event::Logout { name }).await?;

    Ok(())
}

async fn handle_events(mut event_receiver: Receiver<Event>) -> Result<()> {
    let mut users: HashMap<String, Sender<String>> = HashMap::default();
    let mut writers: HashMap<String, JoinHandle<Result<()>>> = HashMap::default();

    while let Some(event) = event_receiver.next().await {
        match event {
            Event::Message { from, to, msg } => {
                for dest in to {
                    if let Some(sender) = users.get_mut(&dest) {
                        sender.send(format!("from {}: {}\n", from, msg)).await?
                    }
                }
            }
            Event::Login { name, stream } if !users.contains_key(&name) => {
                let (cs, cr) = unbounded::<String>();
                cs.send(format!("Logged in as: {}\n", name)).await?;
                users.insert(name.clone(), cs);
                let writer = task::spawn(handle_writing(stream, cr));
                writers.insert(name.clone(), writer);
            }
            Event::Logout { name } => {
                if users.contains_key(&name) {
                    let user = users.remove(&name).unwrap();
                    user.close();    
                }

                if writers.contains_key(&name) {
                    let writer = writers.remove(&name).unwrap();
                    writer.cancel().await;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

async fn handle_writing(stream: Arc<TcpStream>, mut receiver: Receiver<String>) -> Result<()> {
    let mut stream = &*stream;
    while let Some(msg) = receiver.next().await {
        stream.write_all(msg.as_bytes()).await?;
    }

    Ok(())
}

fn to_event(line: String, stream: Arc<TcpStream>) -> Event {
    #[allow(non_upper_case_globals)]
    static mut name: Option<String> = None;

    if let Some((list, msg)) = line.split_once("->") {
        if unsafe { name.is_none() } {
            return Event::Empty;
        }

        let dest = list
            .split(',')
            .map(|s| s.trim().into())
            .collect::<Vec<String>>();

        return Event::Message {
            from: unsafe { name.clone().unwrap() },
            to: dest,
            msg: msg.into(),
        };
    }

    if line.trim().chars().all(|c| c.is_alphanumeric()) {
        unsafe {
            name.replace(line.trim().to_lowercase().into());
        }

        return Event::Login {
            name: unsafe { name.clone().unwrap() },
            stream,
        };
    }

    Event::Empty
}
