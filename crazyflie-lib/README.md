# Crazyflie lib

Rust Crate to connect and control a [Crazyflie]. This crate is still very much work in progress,
not all Crazyflie functionalities are implemented. The current state should be good enough
to implement a clone of the Crazyflie client's [flight tab].

## Status

The following subsystems are or need to be implemented:

 - [x] Basic Roll Pitch Yaw setpoint
 - [x] Log subsystem
 - [x] Param subsystem
 - [ ] Generic setpoints
 - [ ] Memory subsystem
 - [ ] High-level commander

[Crazyflie]: https://www.bitcraze.io/products/crazyflie-2-1/
[Flight tab]: https://www.bitcraze.io/documentation/repository/crazyflie-clients-python/master/userguides/userguide_client/flightcontrol_tab/