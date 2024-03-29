use std::sync::Arc;

use command_request::RequestData;
use tracing::debug;

use crate::*;

mod command_service;

/// 对 Command 的处理的抽象
pub trait CommandService {
    /// 处理 Command，返回 Response
    fn execute(self, store: &impl Storage) -> CommandResponse;
}

/// Service 数据结构
pub struct Service<Store = MemTable> {
    inner: Arc<ServiceInner<Store>>,
}

impl<Store> Clone for Service<Store> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<Store: Storage> Service<Store> {
    pub fn after_send(&self) {
        self.inner.after_send()
    }
}

/// Service 内部数据结构
pub struct ServiceInner<Store> {
    store: Store,
    on_received: Vec<fn(&CommandRequest) -> Option<CommandResponse>>,
    on_executed: Vec<fn(&CommandResponse) -> Option<CommandResponse>>,
    on_before_send: Vec<fn(&mut CommandResponse) -> Option<CommandResponse>>,
    on_after_send: Vec<fn()>,
}

impl<Store: Storage> ServiceInner<Store> {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            on_received: Vec::new(),
            on_executed: Vec::new(),
            on_before_send: Vec::new(),
            on_after_send: Vec::new(),
        }
    }

    pub fn fn_received(mut self, f: fn(&CommandRequest) -> Option<CommandResponse>) -> Self {
        self.on_received.push(f);
        self
    }

    pub fn fn_executed(mut self, f: fn(&CommandResponse) -> Option<CommandResponse>) -> Self {
        self.on_executed.push(f);
        self
    }

    pub fn fn_before_send(
        mut self,
        f: fn(&mut CommandResponse) -> Option<CommandResponse>,
    ) -> Self {
        self.on_before_send.push(f);
        self
    }

    pub fn fn_after_send(mut self, f: fn()) -> Self {
        self.on_after_send.push(f);
        self
    }

    pub fn after_send(&self) {
        self.on_after_send.iter().for_each(|f| f())
    }
}

impl<Store: Storage> From<ServiceInner<Store>> for Service<Store> {
    fn from(inner: ServiceInner<Store>) -> Self {
        Self {
            inner: Arc::new(inner),
        }
    }
}

impl<Store: Storage> Service<Store> {
    pub fn new(store: impl Into<ServiceInner<Store>>) -> Self {
        Self {
            inner: Arc::new(store.into()),
        }
    }

    pub fn execute(&self, cmd: CommandRequest) -> CommandResponse {
        debug!("Got request: {:?}", cmd);
        if let Some(res) = self.inner.on_received.notify(&cmd) {
            return res;
        }
        let mut res = dispatch(cmd, &self.inner.store);
        debug!("Executed response: {:?}", res);
        if let Some(res) = self.inner.on_executed.notify(&res) {
            return res;
        }
        if let Some(res) = self.inner.on_before_send.notify(&mut res) {
            return res;
        }
        if !self.inner.on_before_send.is_empty() {
            debug!("Modified response: {:?}", res);
        }
        res
    }
}

// 从 Request 中得到 Response，目前处理 HGET/HGETALL/HSET
pub fn dispatch(cmd: CommandRequest, store: &impl Storage) -> CommandResponse {
    match cmd.request_data {
        Some(RequestData::Hget(param)) => param.execute(store),
        Some(RequestData::Hgetall(param)) => param.execute(store),
        Some(RequestData::Hset(param)) => param.execute(store),
        None => KvError::InvalidCommand("Request has no data".into()).into(),
        _ => KvError::Internal("Not implemented".into()).into(),
    }
}

/// 事件通知（不可变事件）
pub trait Notify<Arg> {
    fn notify(&self, arg: &Arg) -> Option<CommandResponse>;
}

/// 事件通知（可变事件）
pub trait NotifyMut<Arg> {
    fn notify(&self, arg: &mut Arg) -> Option<CommandResponse>;
}

impl<Arg> Notify<Arg> for Vec<fn(&Arg) -> Option<CommandResponse>> {
    #[inline]
    fn notify(&self, arg: &Arg) -> Option<CommandResponse> {
        for f in self {
            if let Some(res) = f(arg) {
                return Some(res);
            }
        }
        None
    }
}

impl<Arg> NotifyMut<Arg> for Vec<fn(&mut Arg) -> Option<CommandResponse>> {
    #[inline]
    fn notify(&self, arg: &mut Arg) -> Option<CommandResponse> {
        for f in self {
            if let Some(res) = f(arg) {
                return Some(res);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemTable, Value};

    #[test]
    fn service_should_works() {
        // 我们需要一个 service 结构至少包含 Storage
        let service: Service = ServiceInner::new(MemTable::default()).into();

        // service 可以运行在多线程环境下，它的 clone 应该是轻量级的
        let cloned = service.clone();

        // 创建一个线程，在 table t1 中写入 k1, v1
        let handle = std::thread::spawn(move || {
            let res = cloned.execute(CommandRequest::new_hset("t1", "k1", "v1".into()));
            assert_res_ok(res, &[Value::default()], &[]);
        });
        handle.join().unwrap();

        // 在当前线程下读取 table t1 的 k1，应该返回 v1
        let res = service.execute(CommandRequest::new_hget("t1", "k1"));
        assert_res_ok(res, &["v1".into()], &[]);
    }
}

#[cfg(test)]
use crate::{KvPair, Value};

// 测试成功返回的结果
#[cfg(test)]
pub fn assert_res_ok(mut res: CommandResponse, values: &[Value], pairs: &[KvPair]) {
    res.pairs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(res.status, 200);
    assert_eq!(res.message, "");
    assert_eq!(res.values, values);
    assert_eq!(res.pairs, pairs);
}

// 测试失败返回的结果
#[cfg(test)]
pub fn assert_res_error(res: CommandResponse, code: u32, msg: &str) {
    assert_eq!(res.status, code);
    assert!(res.message.contains(msg));
    assert_eq!(res.values, &[]);
    assert_eq!(res.pairs, &[]);
}

#[test]
fn event_registration_should_work() {
    use http::StatusCode;
    fn b(cmd: &CommandRequest) -> Option<CommandResponse> {
        tracing::info!("Got {:?}", cmd);
        None
    }
    fn c(res: &CommandResponse) -> Option<CommandResponse> {
        tracing::info!("{:?}", res);
        None
    }
    fn d(res: &mut CommandResponse) -> Option<CommandResponse> {
        res.status = StatusCode::CREATED.as_u16() as _;
        None
    }
    fn e() {
        tracing::info!("Data is sent");
    }

    let service: Service = ServiceInner::new(MemTable::default())
        .fn_received(|_: &CommandRequest| None)
        .fn_received(b)
        .fn_executed(c)
        .fn_before_send(d)
        .fn_after_send(e)
        .into();

    let res = service.execute(CommandRequest::new_hset("t1", "k1", "v1".into()));
    assert_eq!(res.status, StatusCode::CREATED.as_u16() as _);
    assert_eq!(res.message, "");
    assert_eq!(res.values, vec![Value::default()]);
}
