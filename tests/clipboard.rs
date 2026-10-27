//! Shared clipboard end to end: a real server and client over encrypted
//! loopback UDP, with fake clipboards standing in for Windows'.

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::*;
use glide::clip::sync::{ClipSync, Clipboard, OnCopy, OpenClipboard};
use glide::clip::ClipData;
use glide::server::ServerClipNet;

#[derive(Clone, Default)]
struct FakeBoard {
    received: Arc<Mutex<Vec<ClipData>>>,
    on_copy: Arc<Mutex<Option<OnCopy>>>,
}

impl Clipboard for FakeBoard {
    fn set(&self, data: ClipData) {
        self.received.lock().unwrap().push(data);
    }
}

impl FakeBoard {
    fn opener(&self) -> OpenClipboard {
        let b = self.clone();
        Box::new(move |on_copy| {
            *b.on_copy.lock().unwrap() = Some(on_copy);
            Ok(Box::new(b) as Box<dyn Clipboard>)
        })
    }
    fn copy(&self, d: ClipData) {
        let t = Instant::now();
        while self.on_copy.lock().unwrap().is_none() {
            assert!(t.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(5));
        }
        (self.on_copy.lock().unwrap().as_ref().unwrap())(d);
    }
    fn wait_for(&self, n: usize) -> ClipData {
        let t = Instant::now();
        while self.received.lock().unwrap().len() < n {
            assert!(t.elapsed() < Duration::from_secs(10), "clipboard never arrived");
            std::thread::sleep(Duration::from_millis(5));
        }
        self.received.lock().unwrap()[n - 1].clone()
    }
}

#[test]
fn copy_on_one_pc_paste_on_the_other() {
    let p = connected_pair(classic());
    let client = p.client.as_ref().unwrap();

    let server_board = FakeBoard::default();
    let server_clip = ClipSync::start(true, Box::new(ServerClipNet::new(&p.server)), server_board.opener());
    p.server.set_clip_inbox(Some(server_clip.inbox()));

    let client_board = FakeBoard::default();
    let client_clip = ClipSync::start(true, Box::new(client.audio_sender().unwrap()), client_board.opener());
    client.set_clip_inbox(Some(client_clip.inbox()));

    // Copy text on the sharing PC: it lands on the laptop.
    let text = ClipData {
        text: Some("copied on the main PC".into()),
        html: Some(b"Version:0.9\r\n<b>bold</b>".to_vec()),
        ..Default::default()
    };
    server_board.copy(text.clone());
    assert_eq!(client_board.wait_for(1), text);

    // Copy a screenshot-sized picture on the laptop: it lands on the main PC.
    let picture: Vec<u8> = (0..2_000_000u32)
        .map(|i| ((i / 3) % 256) as u8 ^ (i >> 14) as u8)
        .collect();
    let image = ClipData {
        image: Some(picture),
        ..Default::default()
    };
    let t = Instant::now();
    client_board.copy(image.clone());
    assert_eq!(server_board.wait_for(1), image);
    eprintln!("2 MB picture arrived in {:?}", t.elapsed());

    // It wasn't bounced back to where it came from.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(client_board.received.lock().unwrap().len(), 1);
    assert_eq!(server_board.received.lock().unwrap().len(), 1);

    // Turned off on the laptop: copies there stay there.
    client_clip.set_enabled(false);
    client_board.copy(ClipData {
        text: Some("private".into()),
        ..Default::default()
    });
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(server_board.received.lock().unwrap().len(), 1);

    p.server.set_clip_inbox(None);
    client.set_clip_inbox(None);
}
