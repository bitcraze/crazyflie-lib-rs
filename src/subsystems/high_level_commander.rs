//! # High-level commander subsystem
//!
//! This subsystem is responsible for managing high-level commands and setpoints for the Crazyflie.
//! It builds on top of the (low-level) [`crate::subsystems::commander::Commander`] subsystem and provides a more user-friendly interface
//! for controlling the drone's behavior.

use crazyflie_link::Packet;
use flume::Sender;

use crate::{Error, Result};

use crate::crazyflie::HL_COMMANDER_PORT;


// Command type identifiers
const COMMAND_SET_GROUP_MASK: u8 = 0;
const COMMAND_STOP: u8 = 3;
const COMMAND_DEFINE_TRAJECTORY: u8 = 6;
const COMMAND_TAKEOFF_2: u8 = 7;
const COMMAND_LAND_2: u8 = 8;
const COMMAND_SPIRAL: u8 = 11;
const COMMAND_GO_TO_2: u8 = 12;
const COMMAND_START_TRAJECTORY_2: u8 = 13;

/// This mask is used to specify that all Crazyflies should respond to the command.
pub const ALL_GROUPS: u8 = 0;

const TRAJECTORY_LOCATION_MEM: u8 = 1;

/// 4D polynomial trajectory
pub const TRAJECTORY_TYPE_POLY4D: u8 = 0;
/// Compressed 4D polynomial trajectory
pub const TRAJECTORY_TYPE_POLY4D_COMPRESSED: u8 = 1;


/// High-level commander interface for a Crazyflie.
///
/// The high-level commander is a firmware module that generates smooth
/// position setpoints from high-level actions such as *take-off*, *go-to*,
/// *spiral*, and *land*. Internally it plans trajectories (polynomial-based)
/// that are executed by the Crazyflie.
///
/// This Rust type provides an asynchronous, remote client for that module:
/// it builds and sends the required packets over the high-level commander port,
/// exposing a small set of ergonomic methods. When using trajectory functions,
/// ensure the trajectory data has been uploaded to the Crazyflie’s memory first.
///
/// # Notes
/// The high-level commander can be preempted at any time by setpoints from the commander.
/// To return control to the high-level commander, see [`crate::subsystems::commander::Commander::notify_setpoint_stop`].
///
/// A `HighLevelCommander` is typically obtained from a [`crate::Crazyflie`] instance.
///
/// # Safe usage pattern
/// ```no_run
/// # use crazyflie_link::LinkContext;
/// # use crazyflie_lib::Crazyflie;
/// # use tokio::time::{sleep, Duration};
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// # let context = LinkContext::new();
/// # let cf = Crazyflie::connect_from_uri(&context, "radio://0/80/2M/E7E7E7E7E7").await?;
/// // Continue flight sequence even if commands fail
/// let take_off_duration = 2.0;
/// if let Err(e) = cf.high_level_commander.take_off(0.5, None, take_off_duration, None).await {
///     eprintln!("Take-off failed: {e}");
/// }
/// sleep(Duration::from_secs_f32(take_off_duration)).await;
///
/// let land_duration = 2.0;
/// if let Err(e) = cf.high_level_commander.land(0.0, None, land_duration, None).await {
///     eprintln!("Landing failed: {e}");
/// }
/// sleep(Duration::from_secs_f32(land_duration)).await;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct HighLevelCommander {
    uplink: Sender<Packet>,
}

/// Constructor methods.
impl HighLevelCommander {
    /// Create a new HighLevelCommander
    pub fn new(uplink: Sender<Packet>) -> Self {
        Self { uplink }
    }
}

/// Group mask related commands.
impl HighLevelCommander {
    /// Set the group mask for the high-level commander.
    ///
    /// # Arguments
    /// * `group_mask` - The group mask to set. Use `ALL_GROUPS` to set the mask for all Crazyflies.
    pub async fn set_group_mask(&self, group_mask: u8) -> Result<()> {
        let mut payload = Vec::with_capacity(2);
        payload.push(COMMAND_SET_GROUP_MASK);
        payload.push(group_mask);

        let pk = Packet::new(HL_COMMANDER_PORT, 0, payload);

        self.uplink
            .send_async(pk)
            .await
            .map_err(|_| Error::Disconnected)?;
        Ok(())
    }
}

/// High-level movement commands.
///
/// # Warning
/// Avoid overlapping movement commands. When a command is sent to a Crazyflie
/// while another is currently executing, the generated polynomial can take
/// unexpected routes and have high peaks.
impl HighLevelCommander {
    /// Take off vertically from the current x-y position to the given target height.
    ///
    /// # Arguments
    /// * `height` - Target height (meters) above the world origin.
    /// * `yaw` - Target yaw (radians). Use `None` to maintain the current yaw.
    /// * `duration` - Time (seconds) to reach the target height.
    /// * `group_mask` - Bitmask selecting which Crazyflies to command. Use `None` for all Crazyflies.
    pub async fn take_off(&self, height: f32, yaw: Option<f32>, duration: f32, group_mask: Option<u8>) -> Result<()> {
        let use_current_yaw = yaw.is_none();
        let target_yaw = yaw.unwrap_or(0.0);

        let group_mask_value = group_mask.unwrap_or(ALL_GROUPS);

        let mut payload = Vec::with_capacity(3 + 3 * 4);
        payload.push(COMMAND_TAKEOFF_2);
        payload.push(group_mask_value);
        payload.extend_from_slice(&height.to_le_bytes());
        payload.extend_from_slice(&target_yaw.to_le_bytes());
        payload.push(use_current_yaw as u8);
        payload.extend_from_slice(&duration.to_le_bytes());

        let pk = Packet::new(HL_COMMANDER_PORT, 0, payload);

        self.uplink
            .send_async(pk)
            .await
           .map_err(|_| Error::Disconnected)?;
        Ok(())
    }

    /// Land vertically from the current x-y position to the given target height.
    ///
    /// # Arguments
    /// * `height` - Target height (meters) above the world origin.
    /// * `yaw` - Target yaw (radians). Use `None` to maintain the current yaw.
    /// * `duration` - Time (seconds) to reach the target height.
    /// * `group_mask` - Bitmask selecting which Crazyflies to command. Use `None` for all Crazyflies.
    pub async fn land(&self, height: f32, yaw: Option<f32>, duration: f32, group_mask: Option<u8>) -> Result<()> {
        let use_current_yaw = yaw.is_none();
        let target_yaw = yaw.unwrap_or(0.0);

        let group_mask_value = group_mask.unwrap_or(ALL_GROUPS);

        let mut payload = Vec::with_capacity(3 + 3 * 4);
        payload.push(COMMAND_LAND_2);
        payload.push(group_mask_value);
        payload.extend_from_slice(&height.to_le_bytes());
        payload.extend_from_slice(&target_yaw.to_le_bytes());
        payload.push(use_current_yaw as u8);
        payload.extend_from_slice(&duration.to_le_bytes());

        let pk = Packet::new(HL_COMMANDER_PORT, 0, payload);

        self.uplink
            .send_async(pk)
            .await
            .map_err(|_| Error::Disconnected)?;
        Ok(())
    }

    /// Stop the current high-level command and disable motors.
    ///
    /// This immediately halts any active high-level command (takeoff, land, go_to, spiral, 
    /// or trajectory execution) and stops motor output.
    ///
    /// # Arguments
    /// * `group_mask` - Bitmask selecting which Crazyflies to command. Use `None` for all Crazyflies.
    pub async fn stop(&self, group_mask: Option<u8>) -> Result<()> {
        let group_mask_value = group_mask.unwrap_or(ALL_GROUPS);

        let mut payload = Vec::with_capacity(2);
        payload.push(COMMAND_STOP);
        payload.push(group_mask_value);

        let pk = Packet::new(HL_COMMANDER_PORT, 0, payload);

        self.uplink
            .send_async(pk)
            .await
            .map_err(|_| Error::Disconnected)?;
        Ok(())
    }

    /// Move to an absolute or relative position with smooth path planning.
    ///
    /// The path is designed to transition smoothly from the current state to the target
    /// position, gradually decelerating at the goal with minimal overshoot. When the 
    /// system is at hover, the path will be a straight line, but if there is any initial
    /// velocity, the path will be a smooth curve.
    ///
    /// The trajectory is derived by solving for a unique 7th-degree polynomial that
    /// satisfies the initial conditions of position, velocity, and acceleration, and
    /// ends at the goal with zero velocity and acceleration. Additionally, the jerk
    /// (derivative of acceleration) is constrained to be zero at both the starting
    /// and ending points.
    ///
    /// # Arguments
    /// * `x` - Target x-position in meters
    /// * `y` - Target y-position in meters
    /// * `z` - Target z-position in meters
    /// * `yaw` - Target yaw angle in radians
    /// * `duration` - Time in seconds to reach the target position
    /// * `relative` - If `true`, positions and yaw are relative to current position; if `false`, absolute
    /// * `linear` - If `true`, use linear interpolation; if `false`, use polynomial trajectory
    /// * `group_mask` - Bitmask selecting which Crazyflies to command. Use `None` for all Crazyflies.
    pub async fn go_to(&self, x: f32, y: f32, z: f32, yaw: f32, duration: f32, relative: bool, linear: bool, group_mask: Option<u8>) -> Result<()> {
        let group_mask_value = group_mask.unwrap_or(ALL_GROUPS);

        let mut payload = Vec::with_capacity(4 + 5 * 4);
        payload.push(COMMAND_GO_TO_2);
        payload.push(group_mask_value);
        payload.push(relative as u8);
        payload.push(linear as u8);
        payload.extend_from_slice(&x.to_le_bytes());
        payload.extend_from_slice(&y.to_le_bytes());
        payload.extend_from_slice(&z.to_le_bytes());
        payload.extend_from_slice(&yaw.to_le_bytes());
        payload.extend_from_slice(&duration.to_le_bytes());

        let pk = Packet::new(HL_COMMANDER_PORT, 0, payload);

        self.uplink
            .send_async(pk)
            .await
            .map_err(|_| Error::Disconnected)?;
        Ok(())
    }

    /// Fly a spiral segment.
    ///
    /// The Crazyflie moves along an arc around a computed center point, sweeping
    /// through an angle of up to ±2π (one full turn). While sweeping, the radius
    /// changes linearly from `initial_radius` to `final_radius`. If the radii are
    /// equal, the path is a circular arc; if they differ, the path spirals inward
    /// or outward accordingly. Altitude changes linearly by `altitude_gain` over
    /// the duration.
    ///
    /// # Center placement
    /// The spiral center is placed differently depending on `sideways` and `clockwise`:
    /// * `sideways = false`
    ///   * `clockwise = true`  → center lies to the **right** of the current heading.
    ///   * `clockwise = false` → center lies to the **left** of the current heading.
    /// * `sideways = true`
    ///   * `clockwise = true`  → center lies **ahead** of the current heading.
    ///   * `clockwise = false` → center lies **behind** the current heading.
    ///
    /// # Orientation
    /// * `sideways = false`: the Crazyflie’s heading follows the tangent of the
    ///   spiral (flies forward along the path).
    /// * `sideways = true`: the Crazyflie’s heading points toward the spiral center
    ///   while circling around it (flies sideways along the path).
    ///
    /// # Direction conventions
    /// * `clockwise` chooses on which side the center is placed.
    /// * The **sign of `angle`** sets the travel direction along the arc:
    ///   `angle > 0` sweeps one way; `angle < 0` traverses the arc in the opposite
    ///   direction (i.e., “backwards”). This can make some combinations appear
    ///   counterintuitive—for example, `sideways = false`, `clockwise = true`,
    ///   `angle < 0` will *look* counter-clockwise from above.
    ///
    /// # Arguments
    /// * `angle` - Total spiral angle in radians (limited to ±2π).
    /// * `initial_radius` - Starting radius in meters (≥ 0).
    /// * `final_radius` - Ending radius in meters (≥ 0).
    /// * `altitude_gain` - Vertical displacement in meters (positive = climb,
    ///   negative = descent).
    /// * `duration` - Time in seconds to complete the spiral.
    /// * `sideways` - If `true`, heading points toward the spiral center;
    ///   if `false`, heading follows the spiral tangent.
    /// * `clockwise` - If `true`, fly clockwise; otherwise counter-clockwise.
    /// * `group_mask` - Bitmask selecting which Crazyflies this applies to.
    ///
    /// # Errors
    /// Returns [`Error::InvalidArgument`] if any parameters are out of range,
    /// or [`Error::Disconnected`] if the command cannot be sent.
    pub async fn spiral(&self, angle: f32, initial_radius: f32, final_radius: f32, altitude_gain: f32, duration: f32, sideways: bool, clockwise: bool, group_mask: Option<u8>) -> Result<()> {
        // Check if all arguments are within range
        if angle.abs() > 2.0 * std::f32::consts::PI {
            return Err(Error::InvalidArgument("angle out of range".to_string()));
        }
        if initial_radius < 0.0 {
            return Err(Error::InvalidArgument("initial_radius must be >= 0".to_string()));
        }
        if final_radius < 0.0 {
            return Err(Error::InvalidArgument("final_radius must be >= 0".to_string()));
        }

        let group_mask_value = group_mask.unwrap_or(ALL_GROUPS);

        let mut payload = Vec::with_capacity(4 + 5 * 4);
        payload.push(COMMAND_SPIRAL);
        payload.push(group_mask_value);
        payload.push(sideways as u8);
        payload.push(clockwise as u8);
        payload.extend_from_slice(&angle.to_le_bytes());
        payload.extend_from_slice(&initial_radius.to_le_bytes());
        payload.extend_from_slice(&final_radius.to_le_bytes());
        payload.extend_from_slice(&altitude_gain.to_le_bytes());
        payload.extend_from_slice(&duration.to_le_bytes());

        let pk = Packet::new(HL_COMMANDER_PORT, 0, payload);

        self.uplink
            .send_async(pk)
            .await
            .map_err(|_| Error::Disconnected)?;
        Ok(())
    }
}


/// Trajectory implementations
impl HighLevelCommander {
    /// Define a trajectory previously uploaded to memory.
    ///
    /// # Arguments
    /// * `trajectory_id` - Identifier used to reference this trajectory later.
    /// * `memory_offset` - Byte offset into trajectory memory where the data begins.
    /// * `piece_count` - Number of segments (pieces) in the trajectory.
    /// * `trajectory_type` - Type of the trajectory data (e.g. Poly4D).
    ///
    /// # Errors
    /// Returns [`Error::Disconnected`] if the command cannot be sent.
    pub async fn define_trajectory(&self, trajectory_id: u8, memory_offset: u32, num_pieces: u8, trajectory_type: Option<u8>) -> Result<()> {
        let trajectory_type_value = trajectory_type.unwrap_or(TRAJECTORY_TYPE_POLY4D);

        let mut payload = Vec::with_capacity(5 + 1 * 4);
        payload.push(COMMAND_DEFINE_TRAJECTORY);
        payload.push(trajectory_id);
        payload.push(TRAJECTORY_LOCATION_MEM);
        payload.push(trajectory_type_value);
        payload.extend_from_slice(&memory_offset.to_le_bytes());
        payload.push(num_pieces);

        let pk = Packet::new(HL_COMMANDER_PORT, 0, payload);

        self.uplink
            .send_async(pk)
            .await
            .map_err(|_| Error::Disconnected)?;
        Ok(())
    }

    /// Start executing a previously defined trajectory.
    ///
    /// The trajectory is identified by `trajectory_id` and can be modified
    /// at execution time by scaling its speed, shifting its position, aligning
    /// its yaw, or running it in reverse.
    ///
    /// # Arguments
    /// * `trajectory_id` - Identifier of the trajectory (as defined with [`HighLevelCommander::define_trajectory`]).
    /// * `time_scale` - Time scaling factor; `1.0` = original speed,
    ///   values >1.0 slow down, values <1.0 speed up.
    /// * `relative_position` - If `true`, shift trajectory to the current setpoint position.
    /// * `relative_yaw` - If `true`, align trajectory yaw to the current yaw.
    /// * `reversed` - If `true`, execute the trajectory in reverse.
    /// * `group_mask` - Mask selecting which Crazyflies this applies to.
    ///   If `None`, defaults to all Crazyflies.
    ///
    /// # Errors
    /// Returns [`Error::Disconnected`] if the command cannot be sent.
     pub async fn start_trajectory(&self, trajectory_id: u8, time_scale: f32, relative_position: bool, relative_yaw: bool, reversed: bool, group_mask: Option<u8>) -> Result<()> {
        let group_mask_value = group_mask.unwrap_or(ALL_GROUPS);

        let mut payload = Vec::with_capacity(5 + 1 * 4);
        payload.push(COMMAND_START_TRAJECTORY_2);
        payload.push(group_mask_value);
        payload.push(relative_position as u8);
        payload.push(relative_yaw as u8);
        payload.push(reversed as u8);
        payload.push(trajectory_id);
        payload.extend_from_slice(&time_scale.to_le_bytes());

        let pk = Packet::new(HL_COMMANDER_PORT, 0, payload);

        self.uplink
            .send_async(pk)
            .await
            .map_err(|_| Error::Disconnected)?;
        Ok(())
    }
}
