// Copyright 2023 The Matrix.org Foundation C.I.C.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::time::Duration;

use assert_matches::assert_matches;
use eyeball_im::VectorDiff;
use futures_util::StreamExt;
use matrix_sdk::{config::SyncSettings, ruma::MilliSecondsSinceUnixEpoch};
use matrix_sdk_test::{
    async_test, EventBuilder, JoinedRoomBuilder, RoomAccountDataTestEvent, StateTestEvent,
    TimelineTestEvent,
};
use matrix_sdk_ui::timeline::{
    Error as TimelineError, RoomExt, TimelineDetails, TimelineItemContent, VirtualTimelineItem,
};
use ruma::{event_id, events::room::message::MessageType, room_id, uint, user_id};
use serde_json::json;
use wiremock::{
    matchers::{header, method, path_regex},
    Mock, ResponseTemplate,
};

mod echo;
mod pagination;
mod read_receipts;
#[cfg(feature = "experimental-sliding-sync")]
pub(crate) mod sliding_sync;

use crate::{logged_in_client, mock_sync};

#[async_test]
async fn edit() {
    let room_id = room_id!("!a98sd12bjh:example.org");
    let (client, server) = logged_in_client().await;
    let sync_settings = SyncSettings::new().timeout(Duration::from_millis(3000));

    let mut ev_builder = EventBuilder::new();
    ev_builder.add_joined_room(JoinedRoomBuilder::new(room_id));

    mock_sync(&server, ev_builder.build_json_sync_response(), None).await;
    let _response = client.sync_once(sync_settings.clone()).await.unwrap();
    server.reset().await;

    let room = client.get_room(room_id).unwrap();
    let timeline = room.timeline().await;
    let (_, mut timeline_stream) = timeline.subscribe().await;

    ev_builder.add_joined_room(JoinedRoomBuilder::new(room_id).add_timeline_event(
        TimelineTestEvent::Custom(json!({
            "content": {
                "body": "hello",
                "msgtype": "m.text",
            },
            "event_id": "$msda7m:localhost",
            "origin_server_ts": 152037280,
            "sender": "@alice:example.org",
            "type": "m.room.message",
        })),
    ));

    mock_sync(&server, ev_builder.build_json_sync_response(), None).await;
    let _response = client.sync_once(sync_settings.clone()).await.unwrap();
    server.reset().await;

    let _day_divider = assert_matches!(
        timeline_stream.next().await,
        Some(VectorDiff::PushBack { value }) => value
    );
    let first = assert_matches!(
        timeline_stream.next().await,
        Some(VectorDiff::PushBack { value }) => value
    );
    let msg = assert_matches!(
        first.as_event().unwrap().content(),
        TimelineItemContent::Message(msg) => msg
    );
    assert_matches!(msg.msgtype(), MessageType::Text(_));
    assert_matches!(msg.in_reply_to(), None);
    assert!(!msg.is_edited());

    ev_builder.add_joined_room(
        JoinedRoomBuilder::new(room_id)
            .add_timeline_event(TimelineTestEvent::Custom(json!({
                "content": {
                    "body": "Test",
                    "formatted_body": "<em>Test</em>",
                    "msgtype": "m.text",
                    "format": "org.matrix.custom.html",
                },
                "event_id": "$7at8sd:localhost",
                "origin_server_ts": 152038280,
                "sender": "@bob:example.org",
                "type": "m.room.message",
            })))
            .add_timeline_event(TimelineTestEvent::Custom(json!({
                "content": {
                    "body": " * hi",
                    "m.new_content": {
                        "body": "hi",
                        "msgtype": "m.text",
                    },
                    "m.relates_to": {
                        "event_id": "$msda7m:localhost",
                        "rel_type": "m.replace",
                    },
                    "msgtype": "m.text",
                },
                "event_id": "$msda7m2:localhost",
                "origin_server_ts": 159056300,
                "sender": "@alice:example.org",
                "type": "m.room.message",
            }))),
    );

    mock_sync(&server, ev_builder.build_json_sync_response(), None).await;
    let _response = client.sync_once(sync_settings.clone()).await.unwrap();
    server.reset().await;

    let second = assert_matches!(timeline_stream.next().await, Some(VectorDiff::PushBack { value }) => value);
    let item = second.as_event().unwrap();
    assert_eq!(item.timestamp(), MilliSecondsSinceUnixEpoch(uint!(152038280)));
    assert!(item.event_id().is_some());
    assert!(!item.is_own());
    assert!(item.original_json().is_some());

    let msg = assert_matches!(item.content(), TimelineItemContent::Message(msg) => msg);
    assert_matches!(msg.msgtype(), MessageType::Text(_));
    assert_matches!(msg.in_reply_to(), None);
    assert!(!msg.is_edited());

    let edit = assert_matches!(
        timeline_stream.next().await,
        Some(VectorDiff::Set { index: 1, value }) => value
    );
    let edited = assert_matches!(
        edit.as_event().unwrap().content(),
        TimelineItemContent::Message(msg) => msg
    );
    let text = assert_matches!(edited.msgtype(), MessageType::Text(text) => text);
    assert_eq!(text.body, "hi");
    assert_matches!(edited.in_reply_to(), None);
    assert!(edited.is_edited());
}

#[async_test]
async fn reaction() {
    let room_id = room_id!("!a98sd12bjh:example.org");
    let (client, server) = logged_in_client().await;
    let sync_settings = SyncSettings::new().timeout(Duration::from_millis(3000));

    let mut ev_builder = EventBuilder::new();
    ev_builder.add_joined_room(JoinedRoomBuilder::new(room_id));

    mock_sync(&server, ev_builder.build_json_sync_response(), None).await;
    let _response = client.sync_once(sync_settings.clone()).await.unwrap();
    server.reset().await;

    let room = client.get_room(room_id).unwrap();
    let timeline = room.timeline().await;
    let (_, mut timeline_stream) = timeline.subscribe().await;

    ev_builder.add_joined_room(
        JoinedRoomBuilder::new(room_id)
            .add_timeline_event(TimelineTestEvent::Custom(json!({
                "content": {
                    "body": "hello",
                    "msgtype": "m.text",
                },
                "event_id": "$TTvQUp1e17qkw41rBSjpZ",
                "origin_server_ts": 152037280,
                "sender": "@alice:example.org",
                "type": "m.room.message",
            })))
            .add_timeline_event(TimelineTestEvent::Custom(json!({
                "content": {
                    "m.relates_to": {
                        "event_id": "$TTvQUp1e17qkw41rBSjpZ",
                        "key": "👍",
                        "rel_type": "m.annotation",
                    },
                },
                "event_id": "$031IXQRi27504",
                "origin_server_ts": 152038300,
                "sender": "@bob:example.org",
                "type": "m.reaction",
            }))),
    );

    mock_sync(&server, ev_builder.build_json_sync_response(), None).await;
    let _response = client.sync_once(sync_settings.clone()).await.unwrap();
    server.reset().await;

    let _day_divider = assert_matches!(
        timeline_stream.next().await,
        Some(VectorDiff::PushBack { value }) => value
    );
    let message = assert_matches!(
        timeline_stream.next().await,
        Some(VectorDiff::PushBack { value }) => value
    );
    assert_matches!(message.as_event().unwrap().content(), TimelineItemContent::Message(_));

    let updated_message = assert_matches!(
        timeline_stream.next().await,
        Some(VectorDiff::Set { index: 1, value }) => value
    );
    let event_item = updated_message.as_event().unwrap();
    let msg = assert_matches!(event_item.content(), TimelineItemContent::Message(msg) => msg);
    assert!(!msg.is_edited());
    assert_eq!(event_item.reactions().len(), 1);
    let group = &event_item.reactions()["👍"];
    assert_eq!(group.len(), 1);
    let senders: Vec<_> = group.senders().collect();
    assert_eq!(senders.as_slice(), [user_id!("@bob:example.org")]);

    // TODO: After adding raw timeline items, check for one here

    ev_builder.add_joined_room(JoinedRoomBuilder::new(room_id).add_timeline_event(
        TimelineTestEvent::Custom(json!({
            "content": {},
            "redacts": "$031IXQRi27504",
            "event_id": "$N6eUCBc3vu58PL8TobGaVQzM",
            "sender": "@bob:example.org",
            "origin_server_ts": 152037280,
            "type": "m.room.redaction",
        })),
    ));

    mock_sync(&server, ev_builder.build_json_sync_response(), None).await;
    let _response = client.sync_once(sync_settings.clone()).await.unwrap();
    server.reset().await;

    let updated_message = assert_matches!(
        timeline_stream.next().await,
        Some(VectorDiff::Set { index: 1, value }) => value
    );
    let event_item = updated_message.as_event().unwrap();
    let msg = assert_matches!(event_item.content(), TimelineItemContent::Message(msg) => msg);
    assert!(!msg.is_edited());
    assert_eq!(event_item.reactions().len(), 0);
}

#[async_test]
async fn redacted_message() {
    let room_id = room_id!("!a98sd12bjh:example.org");
    let (client, server) = logged_in_client().await;
    let sync_settings = SyncSettings::new().timeout(Duration::from_millis(3000));

    let mut ev_builder = EventBuilder::new();
    ev_builder.add_joined_room(JoinedRoomBuilder::new(room_id));

    mock_sync(&server, ev_builder.build_json_sync_response(), None).await;
    let _response = client.sync_once(sync_settings.clone()).await.unwrap();
    server.reset().await;

    let room = client.get_room(room_id).unwrap();
    let timeline = room.timeline().await;
    let (_, mut timeline_stream) = timeline.subscribe().await;

    ev_builder.add_joined_room(
        JoinedRoomBuilder::new(room_id)
            .add_timeline_event(TimelineTestEvent::Custom(json!({
                "content": {},
                "event_id": "$eeG0HA0FAZ37wP8kXlNkxx3I",
                "origin_server_ts": 152035910,
                "sender": "@alice:example.org",
                "type": "m.room.message",
                "unsigned": {
                    "redacted_because": {
                        "content": {},
                        "redacts": "$eeG0HA0FAZ37wP8kXlNkxx3I",
                        "event_id": "$N6eUCBc3vu58PL8TobGaVQzM",
                        "sender": "@alice:example.org",
                        "origin_server_ts": 152037280,
                        "type": "m.room.redaction",
                    },
                },
            })))
            .add_timeline_event(TimelineTestEvent::Custom(json!({
                "content": {},
                "redacts": "$eeG0HA0FAZ37wP8kXlNkxx3I",
                "event_id": "$N6eUCBc3vu58PL8TobGaVQzM",
                "sender": "@alice:example.org",
                "origin_server_ts": 152037280,
                "type": "m.room.redaction",
            }))),
    );

    mock_sync(&server, ev_builder.build_json_sync_response(), None).await;
    let _response = client.sync_once(sync_settings.clone()).await.unwrap();
    server.reset().await;

    let _day_divider = assert_matches!(
        timeline_stream.next().await,
        Some(VectorDiff::PushBack { value }) => value
    );
    let first = assert_matches!(
        timeline_stream.next().await,
        Some(VectorDiff::PushBack { value }) => value
    );
    assert_matches!(first.as_event().unwrap().content(), TimelineItemContent::RedactedMessage);

    // TODO: After adding raw timeline items, check for one here
}

#[async_test]
async fn read_marker() {
    let room_id = room_id!("!a98sd12bjh:example.org");
    let (client, server) = logged_in_client().await;
    let sync_settings = SyncSettings::new().timeout(Duration::from_millis(3000));

    let mut ev_builder = EventBuilder::new();
    ev_builder.add_joined_room(JoinedRoomBuilder::new(room_id));

    mock_sync(&server, ev_builder.build_json_sync_response(), None).await;
    let _response = client.sync_once(sync_settings.clone()).await.unwrap();
    server.reset().await;

    let room = client.get_room(room_id).unwrap();
    let timeline = room.timeline().await;
    let (_, mut timeline_stream) = timeline.subscribe().await;

    ev_builder.add_joined_room(JoinedRoomBuilder::new(room_id).add_timeline_event(
        TimelineTestEvent::Custom(json!({
            "content": {
                "body": "hello",
                "msgtype": "m.text",
            },
            "event_id": "$someplace:example.org",
            "origin_server_ts": 152037280,
            "sender": "@alice:example.org",
            "type": "m.room.message",
        })),
    ));

    mock_sync(&server, ev_builder.build_json_sync_response(), None).await;
    let _response = client.sync_once(sync_settings.clone()).await.unwrap();
    server.reset().await;

    let _day_divider = assert_matches!(timeline_stream.next().await, Some(VectorDiff::PushBack { value }) => value);
    let message = assert_matches!(timeline_stream.next().await, Some(VectorDiff::PushBack { value }) => value);
    assert_matches!(message.as_event().unwrap().content(), TimelineItemContent::Message(_));

    ev_builder.add_joined_room(
        JoinedRoomBuilder::new(room_id).add_account_data(RoomAccountDataTestEvent::FullyRead),
    );

    mock_sync(&server, ev_builder.build_json_sync_response(), None).await;
    let _response = client.sync_once(sync_settings.clone()).await.unwrap();
    server.reset().await;

    // Nothing should happen, the marker cannot be added at the end.

    ev_builder.add_joined_room(JoinedRoomBuilder::new(room_id).add_timeline_event(
        TimelineTestEvent::Custom(json!({
            "content": {
                "body": "hello to you!",
                "msgtype": "m.text",
            },
            "event_id": "$someotherplace:example.org",
            "origin_server_ts": 152067280,
            "sender": "@bob:example.org",
            "type": "m.room.message",
        })),
    ));

    mock_sync(&server, ev_builder.build_json_sync_response(), None).await;
    let _response = client.sync_once(sync_settings.clone()).await.unwrap();
    server.reset().await;

    let message = assert_matches!(timeline_stream.next().await, Some(VectorDiff::PushBack { value }) => value);
    assert_matches!(message.as_event().unwrap().content(), TimelineItemContent::Message(_));

    let marker = assert_matches!(
        timeline_stream.next().await,
        Some(VectorDiff::Insert { index: 2, value }) => value
    );
    assert_matches!(marker.as_virtual().unwrap(), VirtualTimelineItem::ReadMarker);
}

#[async_test]
async fn in_reply_to_details() {
    let room_id = room_id!("!a98sd12bjh:example.org");
    let (client, server) = logged_in_client().await;
    let sync_settings = SyncSettings::new().timeout(Duration::from_millis(3000));

    let mut ev_builder = EventBuilder::new();
    ev_builder.add_joined_room(JoinedRoomBuilder::new(room_id));

    mock_sync(&server, ev_builder.build_json_sync_response(), None).await;
    let _response = client.sync_once(sync_settings.clone()).await.unwrap();
    server.reset().await;

    let room = client.get_room(room_id).unwrap();
    let timeline = room.timeline().await;
    let (_, mut timeline_stream) = timeline.subscribe().await;

    // The event doesn't exist.
    assert_matches!(
        timeline.fetch_details_for_event(event_id!("$fakeevent")).await,
        Err(TimelineError::RemoteEventNotInTimeline)
    );

    ev_builder.add_joined_room(
        JoinedRoomBuilder::new(room_id)
            .add_timeline_event(TimelineTestEvent::Custom(json!({
                "content": {
                    "body": "hello",
                    "msgtype": "m.text",
                },
                "event_id": "$event1",
                "origin_server_ts": 152037280,
                "sender": "@alice:example.org",
                "type": "m.room.message",
            })))
            .add_timeline_event(TimelineTestEvent::Custom(json!({
                "content": {
                    "body": "hello to you too",
                    "msgtype": "m.text",
                    "m.relates_to": {
                        "m.in_reply_to": {
                            "event_id": "$event1",
                        },
                    },
                },
                "event_id": "$event2",
                "origin_server_ts": 152045456,
                "sender": "@bob:example.org",
                "type": "m.room.message",
            }))),
    );

    mock_sync(&server, ev_builder.build_json_sync_response(), None).await;
    let _response = client.sync_once(sync_settings.clone()).await.unwrap();
    server.reset().await;

    let _day_divider = assert_matches!(timeline_stream.next().await, Some(VectorDiff::PushBack { value }) => value);
    let first = assert_matches!(timeline_stream.next().await, Some(VectorDiff::PushBack { value }) => value);
    assert_matches!(first.as_event().unwrap().content(), TimelineItemContent::Message(_));
    let second = assert_matches!(timeline_stream.next().await, Some(VectorDiff::PushBack { value }) => value);
    let second_event = second.as_event().unwrap();
    let message =
        assert_matches!(second_event.content(), TimelineItemContent::Message(message) => message);
    let in_reply_to = message.in_reply_to().unwrap();
    assert_eq!(in_reply_to.event_id, event_id!("$event1"));
    assert_matches!(in_reply_to.event, TimelineDetails::Ready(_));

    ev_builder.add_joined_room(JoinedRoomBuilder::new(room_id).add_timeline_event(
        TimelineTestEvent::Custom(json!({
            "content": {
                "body": "you were right",
                "msgtype": "m.text",
                "m.relates_to": {
                    "m.in_reply_to": {
                        "event_id": "$remoteevent",
                    },
                },
            },
            "event_id": "$event3",
            "origin_server_ts": 152046694,
            "sender": "@bob:example.org",
            "type": "m.room.message",
        })),
    ));

    mock_sync(&server, ev_builder.build_json_sync_response(), None).await;
    let _response = client.sync_once(sync_settings.clone()).await.unwrap();
    server.reset().await;

    let _read_receipt_update =
        assert_matches!(timeline_stream.next().await, Some(VectorDiff::Set { value, .. }) => value);

    let third = assert_matches!(
        timeline_stream.next().await,
        Some(VectorDiff::PushBack { value }) => value
    );
    let third_event = third.as_event().unwrap();
    let message =
        assert_matches!(third_event.content(), TimelineItemContent::Message(message) => message);
    let in_reply_to = message.in_reply_to().unwrap();
    assert_eq!(in_reply_to.event_id, event_id!("$remoteevent"));
    assert_matches!(in_reply_to.event, TimelineDetails::Unavailable);

    Mock::given(method("GET"))
        .and(path_regex(r"^/_matrix/client/r0/rooms/.*/event/\$remoteevent"))
        .and(header("authorization", "Bearer 1234"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "errcode": "M_NOT_FOUND",
            "error": "Event not found.",
        })))
        .expect(1)
        .mount(&server)
        .await;

    // Fetch details remotely if we can't find them locally.
    timeline.fetch_details_for_event(third_event.event_id().unwrap()).await.unwrap();
    server.reset().await;

    let third = assert_matches!(timeline_stream.next().await, Some(VectorDiff::Set { index: 3, value }) => value);
    let message = assert_matches!(third.as_event().unwrap().content(), TimelineItemContent::Message(message) => message);
    assert_matches!(message.in_reply_to().unwrap().event, TimelineDetails::Pending);

    let third = assert_matches!(timeline_stream.next().await, Some(VectorDiff::Set { index: 3, value }) => value);
    let message = assert_matches!(third.as_event().unwrap().content(), TimelineItemContent::Message(message) => message);
    assert_matches!(message.in_reply_to().unwrap().event, TimelineDetails::Error(_));

    Mock::given(method("GET"))
        .and(path_regex(r"^/_matrix/client/r0/rooms/.*/event/\$remoteevent"))
        .and(header("authorization", "Bearer 1234"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": {
                "body": "Alice is gonna arrive soon",
                "msgtype": "m.text",
            },
            "room_id": room_id,
            "event_id": "$event0",
            "origin_server_ts": 152024004,
            "sender": "@admin:example.org",
            "type": "m.room.message",
        })))
        .expect(1)
        .mount(&server)
        .await;

    timeline.fetch_details_for_event(third_event.event_id().unwrap()).await.unwrap();

    let third = assert_matches!(timeline_stream.next().await, Some(VectorDiff::Set { index: 3, value }) => value);
    let message = assert_matches!(third.as_event().unwrap().content(), TimelineItemContent::Message(message) => message);
    assert_matches!(message.in_reply_to().unwrap().event, TimelineDetails::Pending);

    let third = assert_matches!(timeline_stream.next().await, Some(VectorDiff::Set { index: 3, value }) => value);
    let message = assert_matches!(third.as_event().unwrap().content(), TimelineItemContent::Message(message) => message);
    assert_matches!(message.in_reply_to().unwrap().event, TimelineDetails::Ready(_));
}

#[async_test]
async fn sync_highlighted() {
    let room_id = room_id!("!a98sd12bjh:example.org");
    let (client, server) = logged_in_client().await;
    let sync_settings = SyncSettings::new().timeout(Duration::from_millis(3000));

    let mut ev_builder = EventBuilder::new();
    ev_builder
        // We need the member event and power levels locally so the push rules processor works.
        .add_joined_room(
            JoinedRoomBuilder::new(room_id)
                .add_state_event(StateTestEvent::Member)
                .add_state_event(StateTestEvent::PowerLevels),
        );

    mock_sync(&server, ev_builder.build_json_sync_response(), None).await;
    let _response = client.sync_once(sync_settings.clone()).await.unwrap();
    server.reset().await;

    let room = client.get_room(room_id).unwrap();
    let timeline = room.timeline().await;
    let (_, mut timeline_stream) = timeline.subscribe().await;

    ev_builder.add_joined_room(JoinedRoomBuilder::new(room_id).add_timeline_event(
        TimelineTestEvent::Custom(json!({
            "content": {
                "body": "hello",
                "msgtype": "m.text",
            },
            "event_id": "$msda7m0df9E9op3",
            "origin_server_ts": 152037280,
            "sender": "@example:localhost",
            "type": "m.room.message",
        })),
    ));

    mock_sync(&server, ev_builder.build_json_sync_response(), None).await;
    let _response = client.sync_once(sync_settings.clone()).await.unwrap();
    server.reset().await;

    let _day_divider = assert_matches!(
        timeline_stream.next().await,
        Some(VectorDiff::PushBack { value }) => value
    );
    let first = assert_matches!(
        timeline_stream.next().await,
        Some(VectorDiff::PushBack { value }) => value
    );
    let remote_event = first.as_event().unwrap();
    // Own events don't trigger push rules.
    assert!(!remote_event.is_highlighted());

    ev_builder.add_joined_room(JoinedRoomBuilder::new(room_id).add_timeline_event(
        TimelineTestEvent::Custom(json!({
            "content": {
                "body": "This room has been replaced",
                "replacement_room": "!newroom:localhost",
            },
            "event_id": "$foun39djjod0f",
            "origin_server_ts": 152039280,
            "sender": "@bob:localhost",
            "state_key": "",
            "type": "m.room.tombstone",
        })),
    ));

    mock_sync(&server, ev_builder.build_json_sync_response(), None).await;
    let _response = client.sync_once(sync_settings.clone()).await.unwrap();
    server.reset().await;

    let second = assert_matches!(
        timeline_stream.next().await,
        Some(VectorDiff::PushBack { value }) => value
    );
    let remote_event = second.as_event().unwrap();
    // `m.room.tombstone` should be highlighted by default.
    assert!(remote_event.is_highlighted());
}
