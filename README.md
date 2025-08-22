# Crazyflie lib

Rust Crate to connect and control a [Crazyflie]. This crate is still very much work in progress,
not all Crazyflie functionalities are implemented. The current state should be good enough
to implement a clone of the Crazyflie client's [flight tab].

## Status

The following subsystems are or need to be implemented:

 - [ ] App channel
 - [x] Commander
   - [x] Basic Roll Pitch Yaw setpoint
   - [x] Generic setpoints
 - [x] Console
 - [ ] High-level commander
 - [ ] Localization
 - [x] Log subsystem
 - [ ] Memory subsystem
 - [x] Param subsystem
 - [x] Platform services

The [python Crazyflie lib] implements a brunch of higher-level functionality like [swarm support] helpers. Those are out of scope of this crate and will need to be implemented in another specialized crate.


[Crazyflie]: https://www.bitcraze.io/products/crazyflie-2-1/
[Flight tab]: https://www.bitcraze.io/documentation/repository/crazyflie-clients-python/master/userguides/userguide_client/flightcontrol_tab/
[python Crazyflie lib]: https://github.com/bitcraze/crazyflie-lib-python
[swarm support]: https://www.bitcraze.io/documentation/repository/crazyflie-lib-python/master/api/cflib/crazyflie/swarm/
