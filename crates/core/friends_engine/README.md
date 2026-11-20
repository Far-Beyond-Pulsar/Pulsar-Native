# friends_engine

Friends list, mutual-friend detection and notifications for the editor (the multiplayer friends backend).

- Owns: the friends model, persistence of each user's list and relay home servers in a public GitHub gist, mutual detection, relay room keys and online checks, and the WebSocket notification listener.
- Entry points: `get_friends_list`, `send_friend_request`, `accept_friend_request`, `remove_friend`, `start_notification_listener`, `take_notifications`, `is_authenticated`, `normalize_relay_url`.
- Workspace path dependencies, plus SceneDB (Cargo.toml): `editor_task_queue`, `engine_state`, `pulsar_auth`.
- Layer: kernel. It may depend on kernel and foundation crates only, dev dependencies included, never on a plugin, tool or host crate (`architecture/layers.toml`, `just arch`).
- Must not depend on: plugins, tools or host crates.
