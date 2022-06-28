use tokio::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

#[cfg(feature = "tokio-rt-multi-thread")]
pub fn get_guard<T>(lock: &Mutex<T>) -> MutexGuard<T> {
    // In Async runtime -> Use block_in_place
    tokio::task::block_in_place(|| lock.blocking_lock())
}

#[cfg(not(feature = "tokio-rt-multi-thread"))]
pub fn get_guard<T>(lock: &Mutex<T>) -> MutexGuard<T> {
    lock.blocking_lock()
}

#[cfg(feature = "tokio-rt-multi-thread")]
pub fn get_read_guard<T>(lock: &RwLock<T>) -> RwLockReadGuard<T> {
    // In Async runtime -> Use block_in_place
    tokio::task::block_in_place(|| lock.blocking_read())
}

#[cfg(not(feature = "tokio-rt-multi-thread"))]
pub fn get_read_guard<T>(lock: &RwLock<T>) -> RwLockReadGuard<T> {
    lock.blocking_read()
}

#[cfg(feature = "tokio-rt-multi-thread")]
pub fn get_write_guard<T>(lock: &RwLock<T>) -> RwLockWriteGuard<T> {
    // In Async runtime -> Use block_in_place
    tokio::task::block_in_place(|| lock.blocking_write())
}

#[cfg(not(feature = "tokio-rt-multi-thread"))]
pub fn get_write_guard<T>(lock: &RwLock<T>) -> RwLockWriteGuard<T> {
    lock.blocking_write()
}
