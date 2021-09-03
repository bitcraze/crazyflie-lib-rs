use std::collections::HashMap;
use std::convert::TryInto;
use std::rc::Rc;
use std::rc::Weak;
use std::sync::Arc;

use futures::StreamExt;
use futures::lock::Mutex;

use js_sys::Promise;
use wasm_bindgen::{prelude::*, JsCast};
use wasm_bindgen_futures::JsFuture;

use wasm_bindgen_futures::future_to_promise;
use wasm_bindgen_futures::spawn_local as spawn;

use serde::Serialize;

#[wasm_bindgen]
pub struct Crazyflie {
    link_context: Rc<crazyflie_link::LinkContext>,
    // crazyflie: Rc<Mutex<crazyflie_lib::Crazyflie>>,
    crazyflie: Rc<crazyflie_lib::Crazyflie>,
    commander: Commander,
    param: Param,
    log: Log,
}

#[wasm_bindgen]
impl Crazyflie {
    pub async fn connect(uri: String) -> Result<Crazyflie, JsValue> {
        let link_context = crazyflie_link::LinkContext::new(Arc::new(async_executors::Bindgen));
        let crazyflie = crazyflie_lib::Crazyflie::connect_from_uri(
            async_executors::Bindgen,
            &link_context,
            &uri,
        )
        .await
        .map_err(|e| format!("{:?}", e))?;
        let crazyflie = Rc::new(crazyflie);
        Ok(Crazyflie {
            crazyflie: crazyflie.clone(),
            link_context: Rc::new(link_context),
            commander: Commander {
                crazyflie: Rc::downgrade(&crazyflie),
            },
            param: Param {
                crazyflie: Rc::downgrade(&crazyflie),
            },
            log: Log {
                crazyflie: Rc::downgrade(&crazyflie),
            },
        })
    }

    pub fn disconnect(&self) -> Promise {
        let crazyflie = self.crazyflie.clone();
        wasm_bindgen_futures::future_to_promise(async move {
            crazyflie.disconnect().await;
            Ok(JsValue::NULL)
        })
    }

    pub fn wait_disconnect(&self) -> Promise {
        let crazyflie = self.crazyflie.clone();
        wasm_bindgen_futures::future_to_promise(async move {
            Ok(crazyflie.wait_disconnect().await.into())
        })
    }

    #[wasm_bindgen(getter)]
    pub fn commander(&self) -> Commander {
        self.commander.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn param(&self) -> Param {
        self.param.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn log(&self) -> Log {
        self.log.clone()
    }
}

#[wasm_bindgen]
#[derive(Clone)]
pub struct Commander {
    crazyflie: Weak<crazyflie_lib::Crazyflie>,
}

#[wasm_bindgen]
impl Commander {
    pub fn setpoint_rpyt(&self, roll: f32, pitch: f32, yaw: f32, thrust: u16) -> Promise {
        let crazyflie = self.crazyflie.upgrade();
        wasm_bindgen_futures::future_to_promise(async move {
            let crazyflie = crazyflie.ok_or("Disconnected".to_owned())?;
            crazyflie
                .commander
                .setpoint_rpyt(roll, pitch, yaw, thrust)
                .await;
            Ok(JsValue::NULL)
        })
    }
}

#[wasm_bindgen]
#[derive(Clone)]
pub struct Param {
    crazyflie: Weak<crazyflie_lib::Crazyflie>,
}

#[wasm_bindgen]
impl Param {
    #[wasm_bindgen(getter)]
    pub fn names(&self) -> Result<JsValue, JsValue> {
        let crazyflie = self
            .crazyflie
            .upgrade()
            .ok_or::<JsValue>("Disconnected".into())?;

        let ret = JsValue::from_serde(&crazyflie.param.names()).unwrap();

        Ok(ret)
    }

    pub fn get_type(&self, name: String) -> Result<String, JsValue> {
        let crazyflie = self
            .crazyflie
            .upgrade()
            .ok_or::<JsValue>("Disconnected".into())?;

        let t = crazyflie
            .param
            .get_type(&name)
            .map_err::<JsValue, _>(|e| format!("{:?}", e).into());

        Ok(format!("{:?}", t))
    }

    pub fn get(&self, name: String) -> Promise {
        let crazyflie = self.crazyflie.clone();

        wasm_bindgen_futures::future_to_promise(async move {
            let crazyflie = crazyflie
                .upgrade()
                .ok_or::<JsValue>("Disconnected".into())?;

            crazyflie
                .param
                .get_lossy(&name)
                .await
                .map(|v| v.into())
                .map_err(|e| format!("{:?}", e).into())
        })
    }

    pub fn set(&self, name: String, value: f64) -> Promise {
        let crazyflie = self.crazyflie.clone();

        wasm_bindgen_futures::future_to_promise(async move {
            let crazyflie = crazyflie
                .upgrade()
                .ok_or::<JsValue>("Disconnected".into())?;

            crazyflie
                .param
                .set_lossy(&name, value)
                .await
                .map(|_| JsValue::NULL)
                .map_err::<JsValue, _>(|e| format!("{:?}", e).into())
        })
    }

    pub fn watch_update(&self, callback: &js_sys::Function) -> Result<(), JsValue> {
        let this = JsValue::NULL;
        let crazyflie = self.crazyflie.upgrade().ok_or::<JsValue>("Disconnected".into())?;
        let callback = callback.clone();

        wasm_bindgen_futures::spawn_local(async move {
            let mut watcher = crazyflie.param.watch_change().await;

            while let Some((name, value)) = watcher.next().await {
                let name = JsValue::from_str(&name);
                let value = JsValue::from_f64(value.to_f64_lossy());
                let _ = callback.call2(&this, &name, &value);
            }
        });
        Ok(())
    }
}

#[wasm_bindgen]
#[derive(Clone)]
pub struct Log {
    crazyflie: Weak<crazyflie_lib::Crazyflie>,
}

#[wasm_bindgen]
impl Log {
    #[wasm_bindgen(getter)]
    pub fn names(&self) -> Result<JsValue, JsValue> {
        let crazyflie = self
            .crazyflie
            .upgrade()
            .ok_or::<JsValue>("Disconnected".into())?;

        let ret = JsValue::from_serde(&crazyflie.log.names()).unwrap();

        Ok(ret)
    }

    pub fn create_block(&self) -> Promise {
        let crazyflie = self.crazyflie.upgrade();
        wasm_bindgen_futures::future_to_promise(async move {
            let crazyflie = crazyflie.ok_or("Disconnected".to_owned())?;
            let block = crazyflie
                .log
                .create_block()
                .await
                .map_err(|e| format!("{:?}", e))?;
            Ok(LogBlock {
                block: Rc::new(Mutex::new(block)),
            }
            .into())
        })
    }
}

#[wasm_bindgen]
pub struct LogBlock {
    block: Rc<Mutex<crazyflie_lib::log::LogBlock>>,
}

#[wasm_bindgen]
impl LogBlock {
    pub fn add_variable(&self, name: String) -> Promise {
        let block = self.block.clone();

        future_to_promise(async move {
            let mut block = block.lock().await;
            block
                .add_variable(&name)
                .await
                .map_err(|e| format!("{:?}", e))?;
            Ok(JsValue::NULL)
        })
    }

    pub async fn start(self, period_ms: usize) -> Result<LogStream, JsValue> {
        // If I can lock the mutex, then no more clone of Rc exists ...
        let _ = self.block.lock().await;
        let block = Rc::try_unwrap(self.block).unwrap().into_inner();
        let period = crazyflie_lib::log::LogPeriod::from_millis(period_ms as u64)
            .map_err(|e| format!("{:?}", e))?;
        let stream = block.start(period).await.map_err(|e| format!("{:?}", e))?;
        Ok(LogStream {
            stream: Rc::new(Mutex::new(stream)),
        })
    }
}

#[wasm_bindgen]
pub struct LogStream {
    stream: Rc<Mutex<crazyflie_lib::log::LogStream>>,
}

#[derive(Serialize)]
struct LogData {
    timestamp: u32,
    data: HashMap<String, f64>,
}

#[wasm_bindgen]
impl LogStream {
    pub fn next(&self) -> Promise {
        let stream = self.stream.clone();
        future_to_promise(async move {
            let stream = stream.lock().await;
            let data = stream.next().await.map_err(|e| format!("{:?}", e))?;

            let mut js_data = LogData {
                timestamp: data.timestamp,
                data: HashMap::default(),
            };

            for (name, value) in data.data.into_iter() {
                js_data.data.insert(name, value.to_f64_lossy());
            }

            Ok(JsValue::from_serde(&js_data).unwrap())
        })
    }

    pub async fn stop(self) -> Result<LogBlock, JsValue> {
        // If I can lock the mutex, then no more clone of Rc exists ...
        let _ = self.stream.lock().await;
        let stream = Rc::try_unwrap(self.stream).unwrap().into_inner();
        let block = stream.stop().await.map_err(|e| format!("{:?}", e))?;
        Ok(LogBlock {
            block: Rc::new(Mutex::new(block)),
        })
    }
}

#[wasm_bindgen]
pub async fn scan() -> Result<JsValue, JsValue> {
    let context = crazyflie_link::LinkContext::new(Arc::new(async_executors::Bindgen::new()));

    let uris = context
        .scan([0xe7; 5])
        .await
        .map_err(|e| format!("Scan error: {:?}", e))?;

    Ok(JsValue::from_serde(&uris).unwrap())
}
