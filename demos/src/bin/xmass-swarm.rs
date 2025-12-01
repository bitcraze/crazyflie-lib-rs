// -*- coding: utf-8 -*-
//
//     ||          ____  _ __
//  +------+      / __ )(_) /_______________ _____  ___
//  | 0xBC |     / __  / / __/ ___/ ___/ __ `/_  / / _ \
//  +------+    / /_/ / / /_/ /__/ /  / /_/ / / /_/  __/
//   ||  ||    /_____/_/\__/\___/_/   \__,_/ /___/\___/
//
//  Copyright (C) 2025 Bitcraze AB
//
//  This program is free software; you can redistribute it and/or
//  modify it under the terms of the GNU General Public License
//  as published by the Free Software Foundation; either version 2
//  of the License, or (at your option) any later version.
//
//  This program is distributed in the hope that it will be useful,
//  but WITHOUT ANY WARRANTY; without even the implied warranty of
//  MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
//  GNU General Public License for more details.
//  You should have received a copy of the GNU General Public License
//  along with this program. If not, see <https://www.gnu.org/licenses/>.


// Demo that flies 8 Crazyflies in a Christmas tree pattern
// This demo requires a positioning system. It has been designed for the Lighthouse
// positioning system.

use std::collections::HashMap;
use std::f32::consts::PI;
use std::sync::Arc;
use std::time::Duration;

use crazyflie_lib::{Crazyflie, TocCache};
use tokio::time::sleep;

const URI1: &str = "radio://0/30/2M/E7E7E7E701";
const URI2: &str = "radio://0/30/2M/E7E7E7E702";
const URI3: &str = "radio://0/30/2M/E7E7E7E703";
const URI4: &str = "radio://0/55/2M/E7E7E7E704";
const URI5: &str = "radio://0/55/2M/E7E7E7E705";
const URI6: &str = "radio://0/55/2M/E7E7E7E706";
const URI7: &str = "radio://0/70/2M/E7E7E7E707";
const URI8: &str = "radio://0/70/2M/E7E7E7E708";

// Center of the spiral
const X0: f32 = 0.0;
const Y0: f32 = 0.0;
const Z0: f32 = 0.5;
const X_OFFSET: f32 = 0.4;
const Z_OFFSET: f32 = 0.5;

/// Configuration for each Crazyflie in the swarm
struct CrazyflieConfig {
    x_setpoint: f32,
    z_setpoint: f32,
    takeoff_duration: f32,
    yaw_setpoint: f32,
    rotate_clockwise: bool,
}

fn get_config(uri: &str) -> CrazyflieConfig {
    let (x_setpoint, z_setpoint, yaw_setpoint, rotate_clockwise) = match uri {
        URI1 => (X0 + X_OFFSET, Z0 + 3.0 * Z_OFFSET, PI / 2.0, false),
        URI2 => (X0 + 2.0 * X_OFFSET, Z0 + 2.0 * Z_OFFSET, -PI / 2.0, true),
        URI3 => (X0 + 3.0 * X_OFFSET, Z0 + Z_OFFSET, PI / 2.0, false),
        URI4 => (X0 + 4.0 * X_OFFSET, Z0, -PI / 2.0, true),
        URI5 => (X0 - X_OFFSET, Z0 + 3.0 * Z_OFFSET, -PI / 2.0, false),
        URI6 => (X0 - 2.0 * X_OFFSET, Z0 + 2.0 * Z_OFFSET, PI / 2.0, true),
        URI7 => (X0 - 3.0 * X_OFFSET, Z0 + Z_OFFSET, -PI / 2.0, false),
        URI8 => (X0 - 4.0 * X_OFFSET, Z0, PI / 2.0, true),
        _ => (0.0, Z0, 0.0, false),
    };

    let takeoff_duration = z_setpoint / 0.4;

    CrazyflieConfig {
        x_setpoint,
        z_setpoint,
        takeoff_duration,
        yaw_setpoint,
        rotate_clockwise,
    }
}

/// Returns the positive radius on the right line segment between
/// (0, 2.5) and (2, 0) corresponding to a given z.
fn x_from_z(z: f32) -> f32 {
    2.0 - (4.0 / 5.0) * z
}

async fn arm(cf: &Crazyflie) -> crazyflie_lib::Result<()> {
    cf.platform.send_arming_request(true).await?;
    sleep(Duration::from_secs(1)).await;
    Ok(())
}

async fn run_shared_sequence(cf: &Crazyflie, uri: &str, max_takeoff_duration: f32) -> crazyflie_lib::Result<()> {
    let circle_duration = 8.0;
    let config = get_config(uri);

    println!("Taking off to {}", config.z_setpoint);

    // Takeoff - the high_level_commander.take_off already waits for duration
    cf.high_level_commander
        .take_off(config.z_setpoint, None, config.takeoff_duration, None)
        .await?;
    
    // Wait additional time to sync with other drones
    let extra_wait = max_takeoff_duration - config.takeoff_duration + 1.0;
    if extra_wait > 0.0 {
        sleep(Duration::from_secs_f32(extra_wait)).await;
    }

    // Go to initial position
    cf.high_level_commander
        .go_to(
            config.x_setpoint,
            Y0,
            config.z_setpoint,
            config.yaw_setpoint,
            4.0,
            false,
            false,
            None,
        )
        .await?;
    sleep(Duration::from_secs(1)).await;

    // First spiral - full circle
    cf.high_level_commander
        .spiral(
            2.0 * PI,
            config.x_setpoint.abs(),
            config.x_setpoint.abs(),
            0.0,
            circle_duration,
            false,
            config.rotate_clockwise,
            None,
        )
        .await?;
    sleep(Duration::from_secs(1)).await;

    // Second spiral - half circle, descending
    cf.high_level_commander
        .spiral(
            PI,
            config.x_setpoint.abs(),
            x_from_z(config.z_setpoint - 0.5 * Z_OFFSET),
            -0.5 * Z_OFFSET,
            0.5 * circle_duration,
            false,
            config.rotate_clockwise,
            None,
        )
        .await?;
    sleep(Duration::from_secs_f32(0.5)).await;

    // Third spiral - full circle, ascending
    cf.high_level_commander
        .spiral(
            2.0 * PI,
            x_from_z(config.z_setpoint - 0.5 * Z_OFFSET),
            x_from_z(config.z_setpoint + 0.5 * Z_OFFSET),
            Z_OFFSET,
            circle_duration,
            false,
            config.rotate_clockwise,
            None,
        )
        .await?;
    sleep(Duration::from_secs(1)).await;

    // Fourth spiral - half circle, descending back
    cf.high_level_commander
        .spiral(
            PI,
            x_from_z(config.z_setpoint + 0.5 * Z_OFFSET),
            x_from_z(config.z_setpoint),
            -0.5 * Z_OFFSET,
            0.5 * circle_duration,
            false,
            config.rotate_clockwise,
            None,
        )
        .await?;
    sleep(Duration::from_secs(1)).await;

    // Land
    cf.high_level_commander
        .land(0.0, None, config.takeoff_duration, None)
        .await?;

    // Wait additional time to sync with other drones landing
    let extra_land_wait = max_takeoff_duration - config.takeoff_duration + 3.0;
    if extra_land_wait > 0.0 {
        sleep(Duration::from_secs_f32(extra_land_wait)).await;
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let uris = vec![
        URI1,
        URI2,
        URI3,
        URI4,
        URI5,
        URI6,
        URI7,
        URI8,
    ];

    let config = Config::default();
    let config_toc_cache = ConfigTocCache::new(config, false);

    // Calculate max takeoff duration for synchronization
    let max_takeoff_duration: f32 = uris
        .iter()
        .map(|uri| get_config(uri).takeoff_duration)
        .fold(0.0, f32::max);

    let link_context = crazyflie_link::LinkContext::new();

    // Connect to all Crazyflies
    println!("Connecting to Crazyflies...");
    let mut crazyflies: HashMap<String, Arc<Crazyflie>> = HashMap::new();

    for uri in &uris {
        match Crazyflie::connect_from_uri(&link_context, uri, config_toc_cache.clone()).await {
            Ok(cf) => {
                println!("Connected to {}", uri);
                crazyflies.insert(uri.to_string(), Arc::new(cf));
            }
            Err(e) => {
                eprintln!("Failed to connect to {}: {:?}", uri, e);
            }
        }
    }

    if crazyflies.is_empty() {
        eprintln!("No Crazyflies connected!");
        return Ok(());
    }

    sleep(Duration::from_millis(500)).await;

    // Arm all Crazyflies in parallel
    println!("Arming...");
    let arm_futures: Vec<_> = crazyflies
        .values()
        .map(|cf| {
            let cf = Arc::clone(cf);
            async move { arm(&cf).await }
        })
        .collect();

    futures::future::join_all(arm_futures).await;

    // Run sequence on all Crazyflies in parallel
    println!("Starting sequence...");
    let sequence_futures: Vec<_> = crazyflies
        .iter()
        .map(|(uri, cf)| {
            let cf = Arc::clone(cf);
            let uri = uri.clone();
            async move {
                if let Err(e) = run_shared_sequence(&cf, &uri, max_takeoff_duration).await {
                    eprintln!("Sequence failed for {}: {:?}", uri, e);
                }
            }
        })
        .collect();

    futures::future::join_all(sequence_futures).await;

    sleep(Duration::from_secs(1)).await;

    // Disconnect all Crazyflies
    for (uri, cf) in crazyflies.iter() {
        println!("Disconnecting from {}", uri);
        cf.disconnect().await;
    }

    println!("Done!");
    Ok(())
}




#[derive(Debug, Clone)]
pub struct Config {
    toc_cache: HashMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        println!("No configuration found, loading default values");
        Config {
            toc_cache: HashMap::new(),
        }
    }
}

// Simple in-memory implementation of the TOC Cache
#[derive(Clone)]
struct ConfigTocCache {
    config: Arc<std::sync::Mutex<Config>>,
    no_toc_cache: bool,
}

impl ConfigTocCache {
    fn new(config: Config, no_toc_cache: bool) -> Self {
        ConfigTocCache {
            config: Arc::new(std::sync::Mutex::new(config)),
            no_toc_cache,
        }
    }
}

impl TocCache for ConfigTocCache {
    fn get_toc(&self, crc32: u32) -> Option<String> {
        match self.no_toc_cache {
            true => return None,
            false => self.config.lock().unwrap().toc_cache.get(&crc32.to_string()).cloned(),
        } 
    }
    
    fn store_toc(&self, crc32: u32, toc: &str) {
        match self.no_toc_cache {
            true => return,
            false => {
              let mut config = self.config.lock().unwrap();
              config.toc_cache.insert(crc32.to_string(), toc.to_string());          
            },
        }
    }
}