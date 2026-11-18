use axum::handler::Handler;

use super::*;
use api::graph::ProcessState;
use api::graph::docs::{PostOutside, PostTalk, RoomTalk, TalkTo};

pub(super) fn routes() -> Router<Arc<App>> {
    Router::new()
        .route(
            "/api/rooms/{node}/talk",
            get(room_talk).post(post_room.layer(axum::middleware::from_fn(origin::from_page))),
        )
        .route("/api/rooms/{node}/outside", post(post_outside))
        .route("/rooms/{node}", get(super::settings::index))
}

fn room_node(app: &App, node: NodeId) -> Result<Option<TalkTo>, Problem> {
    match app.graph.talk_to(node) {
        Ok(to) => Ok(Some(to)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(internal(e)),
    }
}

async fn room_talk(
    State(app): State<Arc<App>>,
    Path(node): Path<u32>,
) -> Result<Json<RoomTalk>, Problem> {
    let node = NodeId(node);
    let to = room_node(&app, node)?;
    let lines = app.graph.room_lines(node).map_err(internal)?;
    Ok(Json(RoomTalk { to, lines }))
}

async fn post_room(
    State(app): State<Arc<App>>,
    Path(node): Path<u32>,
    Json(req): Json<PostTalk>,
) -> Result<Json<()>, Problem> {
    let node = NodeId(node);
    let to = room_node(&app, node)?
        .ok_or_else(|| Problem::from(Refusal::NotFound(format!("노드 #{}", node.0))))?;
    if req.text.trim().is_empty() {
        return Err(Refusal::EmptyAnswer.into());
    }
    if matches!(to.process, ProcessState::Stopped { .. }) {
        app.relaunch_by_id(node, crate::graph::Plan::Resume).await?;
    }
    app.graph.post_room(node, &req.text)?;
    app.turn_ends.human(node, Some(&req.text));
    Ok(Json(()))
}

async fn post_outside(
    State(app): State<Arc<App>>,
    Path(node): Path<u32>,
    Json(req): Json<PostOutside>,
) -> Result<Json<u32>, Problem> {
    Ok(Json(app.graph.post_outside(
        NodeId(node),
        &req.from,
        &req.text,
    )?))
}
