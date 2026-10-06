//! Source-side admission holds. No character/inventory data crosses server boundaries.
use crate::{Settings, client::Client};
use common::{comp::{self, Agent, CharacterState, Controller, ForceUpdate, Immovable,
    Object, Pos, Presence, PresenceKind, Vel}, resources::Time, terrain::TerrainGrid, vol::ReadVol};
use common_net::msg::ServerGeneral;
use specs::{Entity, Join, World, WorldExt};
use std::{collections::HashMap, sync::Mutex};
use vek::Vec3;

const HOLD_TIMEOUT: f64 = 60.0;
const RETRY_DELAY: f64 = 10.0;
const ACTIVATION_RADIUS: f32 = 1.75;

struct Hold {
    id: u64,
    portal: Entity,
    origin: Vec3<f32>,
    hover: Vec3<f32>,
    expires: f64,
    cancelled: bool,
    had_immovable: bool,
}
#[derive(Default)]
struct Transfers {
    next_id: u64,
    holds: HashMap<Entity, Hold>,
    cooldowns: HashMap<Entity, f64>,
}
#[derive(Default)]
pub(crate) struct PortalTransfers(Mutex<Transfers>);
impl PortalTransfers {
    pub(crate) fn is_holding(&self, entity: Entity) -> bool {
        self.0.lock().unwrap().holds.contains_key(&entity)
    }
    pub(crate) fn cancel(&self, entity: Entity, id: u64) {
        if let Some(hold) = self.0.lock().unwrap().holds.get_mut(&entity).filter(|h| h.id == id) {
            hold.cancelled = true;
        }
    }
}

/// Fail closed: every support and clearance block must be loaded and valid.
fn clear_landing(terrain: &TerrainGrid, base: Vec3<f32>) -> bool {
    let p = base.map(|v| v.floor() as i32);
    (-3..=3).all(|x| (-3..=3).all(|y| {
        terrain.get(p + Vec3::new(x, y, -1)).is_ok_and(|b| b.is_filled())
            && (0..8).all(|z| terrain.get(p + Vec3::new(x, y, z))
                .is_ok_and(|b| !b.is_filled() && !b.is_liquid()))
    }))
}

pub(crate) fn maintain(ecs: &World) {
    let time = ecs.read_resource::<Time>().0;
    let destination = ecs.read_resource::<Settings>().town_portal_destination.clone();
    let transfers = ecs.read_resource::<PortalTransfers>();
    let mut state = transfers.0.lock().unwrap();
    let entities = ecs.entities();
    let objects = ecs.read_storage::<Object>();
    let clients = ecs.read_storage::<Client>();
    let presences = ecs.read_storage::<Presence>();
    let healths = ecs.read_storage::<comp::Health>();
    let agents = ecs.read_storage::<Agent>();
    let riders = ecs.read_storage::<common::link::Is<common::mounting::Rider>>();
    let volume_riders = ecs.read_storage::<common::link::Is<common::mounting::VolumeRider>>();
    let mut positions = ecs.write_storage::<Pos>();
    let mut velocities = ecs.write_storage::<Vel>();
    let mut controllers = ecs.write_storage::<Controller>();
    let mut updates = ecs.write_storage::<ForceUpdate>();
    let mut immovable = ecs.write_storage::<Immovable>();
    let mut character_states = ecs.write_storage::<CharacterState>();
    let terrain = ecs.read_resource::<TerrainGrid>();
    let portals: Vec<_> = (&entities, &objects, &positions).join()
        .filter(|(_, object, _)| matches!(object, Object::ServerPortal))
        .map(|(e, _, pos)| (e, pos.0)).collect();

    let release: Vec<_> = state.holds.iter().filter(|(entity, hold)| {
        hold.cancelled || time >= hold.expires || destination.is_none()
            || !clients.contains(**entity) || !presences.contains(**entity)
            || !objects.get(hold.portal).is_some_and(|o| matches!(o, Object::ServerPortal))
    }).map(|(e, _)| *e).collect();
    for entity in release {
        let hold = state.holds.remove(&entity).unwrap();
        if let Some(pos) = positions.get_mut(entity) { pos.0 = hold.origin; }
        if let Some(vel) = velocities.get_mut(entity) { *vel = Vel::zero(); }
        if let Some(update) = updates.get_mut(entity) { update.update(); }
        if !hold.had_immovable { immovable.remove(entity); }
        if let Some(controller) = controllers.get_mut(entity) { *controller = Controller::default(); }
        state.cooldowns.insert(entity, time + RETRY_DELAY);
        if let Some(client) = clients.get(entity) {
            let _ = client.send(ServerGeneral::ServerPortalReleased { transfer_id: hold.id });
        }
    }
    state.cooldowns.retain(|e, deadline| entities.is_alive(*e) && *deadline > time);
    if let Some(destination) = destination {
        for (portal, base) in &portals {
            if !clear_landing(&terrain, *base) { continue; }
            let candidates: Vec<_> = (&entities, &positions, &clients, &presences).join()
                .filter(|(e, pos, _, presence)| matches!(presence.kind, PresenceKind::Character(_))
                    && pos.0.distance_squared(*base) < ACTIVATION_RADIUS.powi(2)
                    && !state.holds.contains_key(e) && !state.cooldowns.contains_key(e)
                    && !riders.contains(*e) && !volume_riders.contains(*e)
                    && healths.get(*e).is_some_and(|h| !h.is_dead && !h.has_consumed_death_protection())
                    && character_states.get(*e).is_some_and(|s| matches!(s, CharacterState::Idle(_) | CharacterState::Wielding(_)))
                    && !(&agents, &positions).join().any(|(a, p)| p.0.distance_squared(pos.0) < 200.0_f32.powi(2)
                        && a.target.is_some_and(|t| t.target == *e && t.aggro_on)))
                .map(|(e, pos, _, _)| (e, pos.0)).collect();
            for (entity, origin) in candidates {
                state.next_id = state.next_id.wrapping_add(1).max(1);
                let id = state.next_id;
                // Admission must be offered before the source begins holding the avatar.
                if clients.get(entity).unwrap().send(ServerGeneral::ServerPortalOffer {
                    transfer_id: id, destination: destination.clone(),
                }).is_err() { continue; }
                let had_immovable = immovable.contains(entity);
                let _ = immovable.insert(entity, Immovable);
                state.holds.insert(entity, Hold { id, portal: *portal, origin,
                    hover: *base + Vec3::unit_z() * 1.25, expires: time + HOLD_TIMEOUT,
                    cancelled: false, had_immovable });
            }
        }
    }
    for (entity, hold) in &state.holds {
        if let Some(pos) = positions.get_mut(*entity) { pos.0 = hold.hover; }
        if let Some(vel) = velocities.get_mut(*entity) { *vel = Vel::zero(); }
        if let Some(controller) = controllers.get_mut(*entity) { *controller = Controller::default(); }
        if let Some(character_state) = character_states.get_mut(*entity) {
            *character_state = CharacterState::Idle(Default::default());
        }
        if let Some(update) = updates.get_mut(*entity) { update.update(); }
    }
}
