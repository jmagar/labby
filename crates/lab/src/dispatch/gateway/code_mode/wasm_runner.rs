//! Standalone wasmtime smoke-runner used by Code Mode tests and future backends.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use wasmtime::{Config, Engine, Instance, Module, Store, Trap};

pub const DEFAULT_SEARCH_FUEL: u64 = 10_000_000;
pub const DEFAULT_EXECUTE_FUEL: u64 = 50_000_000;
static ENGINE: LazyLock<Result<Engine, String>> = LazyLock::new(|| {
    let mut config = Config::new();
    config.consume_fuel(true);
    config.epoch_interruption(true);
    Engine::new(&config).map_err(|err| err.to_string())
});
static MODULE_CACHE: LazyLock<Mutex<HashMap<String, Arc<Module>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn engine() -> Result<Engine, wasmtime::Error> {
    match ENGINE.as_ref() {
        Ok(engine) => Ok(engine.clone()),
        Err(message) => Err(wasmtime::Error::msg(message.clone())),
    }
}

fn cached_module(engine: &Engine, wat: &str) -> Result<Arc<Module>, wasmtime::Error> {
    let mut cache = MODULE_CACHE
        .lock()
        .map_err(|_| wasmtime::Error::msg("wasm module cache lock poisoned"))?;
    if let Some(module) = cache.get(wat) {
        return Ok(Arc::clone(module));
    }
    let module = Arc::new(Module::new(engine, wat)?);
    cache.insert(wat.to_string(), Arc::clone(&module));
    Ok(module)
}

pub fn run_wasm_i32_export_for_smoke(
    wat: &str,
    export_name: &str,
    fuel: u64,
) -> Result<i32, wasmtime::Error> {
    let engine = engine()?;
    let module = cached_module(&engine, wat)?;
    let mut store = Store::new(&engine, ());
    store.set_fuel(fuel)?;
    store.set_epoch_deadline(u64::MAX);
    let instance = Instance::new(&mut store, module.as_ref(), &[])?;
    let func = instance.get_typed_func::<(), i32>(&mut store, export_name)?;
    func.call(&mut store, ())
}

#[cfg(test)]
pub fn cached_module_count_for_tests() -> usize {
    MODULE_CACHE.lock().map(|cache| cache.len()).unwrap_or(0)
}

pub fn trap_kind(error: &wasmtime::Error) -> Option<&'static str> {
    let message = error.to_string();
    if message.contains("fuel") {
        return Some("code_mode_fuel_exhausted");
    }
    if message.contains("epoch") || message.contains("interrupt") {
        return Some("code_mode_timeout");
    }
    let trap = error.downcast_ref::<Trap>()?;
    match trap {
        Trap::OutOfFuel => Some("code_mode_fuel_exhausted"),
        Trap::Interrupt => Some("code_mode_timeout"),
        _ => Some("server_error"),
    }
}
