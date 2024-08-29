#![feature(try_blocks)]
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use anyhow::anyhow;
use futures::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio::sync::broadcast::Sender;
use tokio_util::codec::{FramedRead, FramedWrite, LinesCodec};

#[path="lib.rs"]
mod lib;
use lib::random_name;


const HELP_MSG: &str = include_str!("help.txt");
const MAIN: &str = "main";

#[derive(Clone)]
struct Names(Arc<Mutex<HashSet<String>>>);

impl Names {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(HashSet::new())))
    }

    fn insert(&self, name: String) -> bool {
        self.0.lock().unwrap().insert(name)
    }

    fn remove(&self, name: &str) {
        self.0.lock().unwrap().remove(name);
    }

    fn get_unique(&self) -> String {
        let mut name = random_name();
        let mut guard = self.0.lock().unwrap();
        while guard.contains(&name) {
            name = random_name();
        }
        guard.insert(name.clone());
        name
    }

    fn to_string(&self) -> String {
        let guard = self.0.lock().unwrap();
        let mut output = String::new();
        for name in guard.iter() {
            output.push_str(&format!("{name} "));
        }
        output
    }
}

struct Room {
    tx: Sender<String>,
    users: HashSet<String>,
}

impl Room {
    fn new() -> Self {
        let (tx, _) = broadcast::channel(16);
        let users = HashSet::new();
        Self {tx, users}
    }

    fn join(&mut self, name: &str) {
        self.users.insert(name.to_string());
    }

    fn leave(&mut self, name: &str) {
        self.users.remove(name);
    }

    fn to_string(&self) -> String {
        let mut ret = String::new();
        for user_name in &self.users {
            ret.push_str(&format!("{user_name} "));
        }
        ret
    }
}

#[derive(Clone)]
struct Rooms(Arc<RwLock<HashMap<String, Room>>>);

impl Rooms {
    fn new() -> Self {
        Self(Arc::new(RwLock::new(HashMap::new())))
    }

    fn join(&self, user_name: &str, room_name: &str) -> Sender<String> {
        let mut write_guard = self.0.write().unwrap();
        // check if room already exists
        if let Some(room) = write_guard.get_mut(room_name) {
            room.join(user_name);
            return room.tx.clone()
        }
        // or create new room
        let mut new_room = Room::new();
        new_room.join(user_name);
        let sender = new_room.tx.clone();
        write_guard.insert(room_name.to_owned(), new_room);
        sender
    }

    fn leave(&self, user_name: &str, room_name: &str) {
        let mut write_guard = self.0.write().unwrap();
        let room = write_guard.get_mut(room_name).unwrap();
        room.leave(user_name);
        if write_guard.get(room_name).unwrap().tx.receiver_count() <= 1 {
            write_guard.remove(room_name);
        }
    }

    fn change(&self, user_name: &str, prev_room: &str, next_room: &str) -> Sender<String> {
        self.leave(user_name, prev_room);
        self.join(user_name, next_room)
    }

    fn change_name(&self, user_name: &str, new_name: &str, room_name: &str) {
        let mut write_guard = self.0.write().unwrap();
        if let Some(room) = write_guard.get_mut(room_name) {
            room.leave(user_name);
            room.join(new_name);
        }
    }

    // You can't use get_room! Because we've put a lock on Rooms,how can we then send a ref of Room outside?
    // fn get_room(&self, room_name: &str) -> Option<&Room> {
    //     let read_guard = self.0.read().unwrap();
    //     read_guard.get(room_name)
    // }

    fn get_users(&self, room_name: &str) -> String {
        let read_guard = self.0.read().unwrap();
        read_guard.get(room_name).unwrap().to_string()
    }

    fn to_string(&self) -> String {
        let read_guard = self.0.read().unwrap();

        // dbg!(read_guard.get("stress-test").unwrap().tx.receiver_count());
        let mut rooms_vec: Vec<(String, usize)> = read_guard
            .iter()
            .map(|(name, room)| (
                name.to_owned(),
                room.tx.receiver_count()
            ))
            .collect();

        rooms_vec.sort_by(|a, b| {
            use std::cmp::Ordering::*;
            match b.1.cmp(&a.1) {
                Equal => a.0.cmp(&b.0),
                ordering => ordering,
            }
        });

        let mut ret = String::new();
        for (room_name, count) in rooms_vec {
            ret += &format!("{room_name}({count}) ");
        }
        ret
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let server = TcpListener::bind("127.0.0.1:42069").await?;

    // init guarded names hashset
    let names = Names::new();
    // init guarded rooms hashmap
    let rooms = Rooms::new();

    // loop to handle multiple connections
    loop {
        let (tcp, _) = server.accept().await?;
        tokio::spawn(handle_user(tcp, names.clone(), rooms.clone()));
    }
}

async fn handle_user(
    mut tcp: TcpStream,
    names: Names,
    rooms: Rooms,
) -> anyhow::Result<()> {
    let (reader, writer) = tcp.split();
    let mut stream = FramedRead::new(reader, LinesCodec::new());
    let mut sink = FramedWrite::new(writer, LinesCodec::new());

    sink.send("Welcome!").await?;
    // assign each user a random name
    let mut name = names.get_unique();
    sink.send(format!("You are {name}")).await?;
    // sends help message as soon as users connect
    sink.send(HELP_MSG).await?;

    let mut room_name= MAIN.to_owned();
    // join main room
    let mut room_tx = rooms.join(&name, &room_name);
    // register a receiver that receives messages for each chat
    let mut room_rx = room_tx.subscribe();
    // notify others when join
    room_tx.send(format!("{name} joined main room"))?;

    let result: anyhow::Result<()> = try {
        loop {
            tokio::select! {
                user_msg = stream.next() => {
                    let user_msg = match user_msg {
                        Some(s) => s?,
                        None => Err(anyhow!("msg is None"))?,
                    };

                    if user_msg == "/help" {
                        sink.send(HELP_MSG).await?;
                    } else if user_msg == "/quit" {
                        break;
                    } else if user_msg == "/print" {
                        sink.send(names.to_string()).await?;
                    } else if user_msg == "/rooms" {
                        let mut msg = String::from("Rooms: ");
                        msg.push_str(&rooms.to_string());
                        sink.send(msg).await?;
                    } else if user_msg == "/users" {
                        let mut msg = format!("Users in {room_name}: ");
                        let users = rooms.get_users(&room_name);
                        msg.push_str(&users.to_string());
                        sink.send(msg).await?;
                    } else if user_msg.starts_with("/join") {
                        let new_room_name = user_msg.split(' ').nth(1).unwrap().to_owned();
                        if new_room_name == room_name {
                            sink.send(format!("you are already in {room_name}")).await?;
                        } else {
                            room_tx.send(format!("{name} left {room_name} room"))?;
                            room_tx = rooms.change(&name, &room_name, &new_room_name);
                            room_rx = room_tx.subscribe();
                            room_name = new_room_name;
                            room_tx.send(format!("{name} joined {room_name} room"))?;
                        }
                    } else if user_msg.starts_with("/name") {
                        // use `to_owned` to convert &str to String
                        let new_name = user_msg.split(' ').nth(1).unwrap().to_owned();
                        if names.insert(new_name.clone()) {
                            sink.send(format!("Your name has been changed to {new_name}")).await?;
                            rooms.change_name(&name, &new_name, &room_name);
                            name = new_name;
                        } else {
                            sink.send(format!("This name {new_name} has been used")).await?;
                        }
                    } else {
                        room_tx.send(format!("{name}: {user_msg}"))?;
                    }
                },

                peer_msg = room_rx.recv() => {
                    sink.send(peer_msg?).await?;
                },
            }
        };
    };

    // notify others when leave
    room_tx.send(format!("{name} left {room_name} room"))?;
    rooms.leave(&name, &room_name);
    names.remove(&name);
    result
}

