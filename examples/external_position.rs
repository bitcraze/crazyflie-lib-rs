use crazyflie_lib::Crazyflie;
use crazyflie_link::LinkContext;
use std::convert::TryInto;
use tokio::time::{sleep, Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let context = LinkContext::new();
    let cf = Crazyflie::connect_from_uri(
        &context,
        "radio://0/80/2M/E7E7E7E7E7",
    )
    .await?;

    println!("Connected!");

    cf.param.set("stabilizer.estimator", 2u8).await?;

    // Set the std deviation for the quaternion data pushed into the kalman filter
    // Use the same value as Python example for better orientation sensitivity
    cf.param.set("locSrv.extQuatStdDev", 8.0e-3f32).await?;

    // Reset the estimator
    cf.param.set("kalman.resetEstimation", 1u8).await?;
    sleep(Duration::from_millis(100)).await;
    cf.param.set("kalman.resetEstimation", 0u8).await?;

    // Wait for estimator to switch and stabilize
    sleep(Duration::from_secs(2)).await;

    // Set up logging - use two blocks to avoid capacity limits
    let mut quat_block = cf.log.create_block().await?;
    quat_block.add_variable("stateEstimate.qx").await?;
    quat_block.add_variable("stateEstimate.qy").await?;
    quat_block.add_variable("stateEstimate.qz").await?;
    quat_block.add_variable("stateEstimate.qw").await?;

    let mut pos_block = cf.log.create_block().await?;
    pos_block.add_variable("stateEstimate.x").await?;
    pos_block.add_variable("stateEstimate.y").await?;
    pos_block.add_variable("stateEstimate.z").await?;

    let log_period_ms = 100;
    let update_period_ms = 5; // How often we send pose updates
    let log_every_n_iterations = log_period_ms / update_period_ms; // Log every N pose updates

    let quat_stream = quat_block.start(Duration::from_millis(log_period_ms).try_into()?).await?;
    let pos_stream = pos_block.start(Duration::from_millis(log_period_ms).try_into()?).await?;

    println!("Sending external pose data...");

    for i in 0..5000 {
        let t = i as f32 * 0.01;

        let x = (t).cos() * 0.5;
        let y = (t).sin() * 0.5;
        let z = 0.0;

        // Full orientation - roll, pitch, yaw
        let roll = (t * 50.0).sin() * 0.2;   // Small roll oscillation
        let pitch = (t * 50.0).cos() * 0.15; // Small pitch oscillation
        let yaw = 1.2f32;                  // Steady yaw rotation

        // Convert Euler angles to quaternion using Crazyflie's rpy2quat() convention
        // NOTE: Negate pitch for coordinate system compatibility
        let roll_rad = roll;
        let pitch_rad = -pitch;
        let yaw_rad = yaw;

        // Apply Tait-Bryan ZYX conversion (from Crazyflie math3d.h)
        let cr = (roll_rad / 2.0).cos();
        let sr = (roll_rad / 2.0).sin();
        let cp = (pitch_rad / 2.0).cos();
        let sp = (pitch_rad / 2.0).sin();
        let cy = (yaw_rad / 2.0).cos();
        let sy = (yaw_rad / 2.0).sin();

        let quat = [
            sr * cp * cy - cr * sp * sy, // qx
            cr * sp * cy + sr * cp * sy, // qy
            cr * cp * sy - sr * sp * cy, // qz
            cr * cp * cy + sr * sp * sy  // qw
        ];

        cf.localization.external_pose.send_external_pose([x, y, z], quat).await?;

        // Log every N iterations to avoid blocking
        if i % log_every_n_iterations == 0 {
            let quat_data = quat_stream.next().await?;
            let pos_data = pos_stream.next().await?;

            let state_x = pos_data.data.get("stateEstimate.x").unwrap();
            let state_y = pos_data.data.get("stateEstimate.y").unwrap();
            let state_z = pos_data.data.get("stateEstimate.z").unwrap();
            let state_qx = quat_data.data.get("stateEstimate.qx").unwrap();
            let state_qy = quat_data.data.get("stateEstimate.qy").unwrap();
            let state_qz = quat_data.data.get("stateEstimate.qz").unwrap();
            let state_qw = quat_data.data.get("stateEstimate.qw").unwrap();

            println!("Sent:  pos=[{:.2}, {:.2}, {:.2}] quat=[{:.3}, {:.3}, {:.3}, {:.3}]",
                     x, y, z, quat[0], quat[1], quat[2], quat[3]);
            println!("State: pos=[{:.2}, {:.2}, {:.2}] quat=[{:.3}, {:.3}, {:.3}, {:.3}]",
                     state_x.to_f64_lossy(), state_y.to_f64_lossy(), state_z.to_f64_lossy(),
                     state_qx.to_f64_lossy(), state_qy.to_f64_lossy(), state_qz.to_f64_lossy(), state_qw.to_f64_lossy());
        }

        sleep(Duration::from_millis(update_period_ms as u64)).await;
    }

    Ok(())
}