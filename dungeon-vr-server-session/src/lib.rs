use std::collections::{BTreeMap, HashMap};

use dungeon_vr_server_connection::{Event as ConnectionEvent, ServerConnection};
use dungeon_vr_shared::net_game::{Input, NetGame, PlayerId};
use dungeon_vr_socket::BoundSocket;
use tokio::select;
use tokio::time::{interval, Duration, Interval};

const TICK_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct TickId(u32);

pub struct ServerSession<S: BoundSocket> {
    connection: ServerConnection<S>,
    game: NetGame,
    players: HashMap<PlayerId, BTreeMap<TickId, Vec<Input>>>,
    tick_interval: Interval,
    tick_id: TickId,
}

enum Event {
    Connection(ConnectionEvent),
    Tick,
}

impl<S: BoundSocket> ServerSession<S> {
    pub fn new(connection: ServerConnection<S>) -> Self {
        Self {
            connection,
            game: NetGame::new(),
            players: HashMap::new(),
            tick_interval: interval(TICK_INTERVAL),
            tick_id: TickId(0),
        }
    }

    pub async fn run(&mut self) {
        loop {
            self.run_once().await;
        }
    }

    async fn run_once(&mut self) {
        todo!()
        // let event = select! {
        //     biased;

        //     event = self.connection.event() => Event::Connection(event),

        //     _ = self.tick_interval.tick() => Event::Tick,
        // };

        // match event {
        //     #[cfg(test)]
        //     Event::Connecting(_) => todo!(),
        //     Event::Connection(ConnectionEvent::PlayerConnected { player_id }) => todo!(),
        //     Event::Connection(ConnectionEvent::PlayerDisconnected { player_id }) => todo!(),
        //     Event::Connection(ConnectionEvent::GameData { player_id, data }) => todo!(),
        //     Event::Tick => todo!(),
        // }
    }

    // fn handle_tick(&mut self) {
    //     // Gather the current buffered inputs for this tick from each connection.
    //     let mut player_inputs = BTreeMap::new();
    //     {
    //         if let Some(inputs) = input_buffer.remove(&self.tick_id) {
    //             player_inputs.insert(*player_id, inputs);
    //         }
    //     }
    //     self.game.update(player_inputs);

    //     // TODO: Send (delta) updates.

    //     self.tick_id = TickId(self.tick_id.0 + 1);
    // }
}
