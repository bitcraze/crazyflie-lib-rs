use crazyflie_lib::Value;
use half::f16;
use std::convert::TryInto;

fn main() {
    let a: f16 = f16::from_f32(32f32);
    let v: Value = a.into();

    let c: f16 = v.try_into().unwrap();

    dbg!(v);
    dbg!(c);

    let d: f32 = v.try_into().unwrap();
    dbg!(d);
}
