use anyhow::{anyhow, bail, Error};
use async_trait::async_trait;
use consul::kv::{KVPair, KV};
use consul::session::{Session, SessionEntry};
use consul::Config;
use futures::future::join_all;
use reqwest::ClientBuilder;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
};
use tracing::{error, warn};

use super::{AdapterId, WorkerId};
use super::{
    AuthorityControl, AuthorityWorkerHeartbeatResponse, GetLeaderResult, LeaderPayload,
    WorkerDescriptor,
};
use crate::{ReadySetError, ReadySetResult};
use noria_errors::internal_err;

pub const WORKER_PREFIX: &str = "workers/";
/// Path to the leader key.
pub const CONTROLLER_KEY: &str = "controller";
/// Path to the controller state.
pub const STATE_KEY: &str = "state";
/// Path to the adapter http endpoints.
pub const ADAPTER_PREFIX: &str = "adapters/";
/// The delay before another client can claim a lock.
const SESSION_LOCK_DELAY: u64 = 0;
/// When the authority releases a session, release the locks held
/// by the session.
const SESSION_RELEASE_BEHAVIOR: &str = "release";
/// The amount of time to wait for a heartbeat before declaring a
/// session as dead.
const SESSION_TTL: &str = "20s";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

struct ConsulAuthorityInner {
    session: Option<SessionEntry>,
    /// The last index that the controller key was modified or
    /// created at.
    controller_index: Option<u64>,
}

/// Coordinator that shares connection information between workers and clients using Consul.
pub struct ConsulAuthority {
    /// The consul client.
    consul: consul::Client,

    /// Deployment associated with this authority.
    deployment: String,

    /// Internal authority state required to handle operations.
    inner: Option<RwLock<ConsulAuthorityInner>>,
}

fn path_to_worker_id(path: &str) -> WorkerId {
    // See `worker_id_to_path` for the type of path this is called on.
    #[allow(clippy::unwrap_used)]
    path[(path.rfind('/').unwrap() + 1)..].to_owned()
}

fn path_to_adapter_id(path: &str) -> AdapterId {
    // See `adapter_id_to_path` for the type of path this is called on.
    #[allow(clippy::unwrap_used)]
    path[(path.rfind('/').unwrap() + 1)..].to_owned()
}

fn worker_id_to_path(id: &str) -> String {
    WORKER_PREFIX.to_owned() + id
}

fn adapter_id_to_path(id: &str) -> String {
    ADAPTER_PREFIX.to_owned() + id
}

impl ConsulAuthority {
    /// The connect string should be in the format of
    /// http(s)://<address>:<port>/<deployment>.
    fn new_with_inner(
        connect_string: &str,
        inner: Option<RwLock<ConsulAuthorityInner>>,
    ) -> ReadySetResult<Self> {
        // We artificially create a namespace for each deployment by prefixing the
        // deployment to each keys path.
        let split_idx = connect_string.rfind('/').ok_or_else(|| {
            ReadySetError::Internal("Consul connect string missing deployment".to_owned())
        })?;

        let deployment = connect_string[(split_idx + 1)..].to_owned();
        let address = connect_string[..split_idx].to_owned();

        let config = ClientBuilder::new()
            .timeout(CONNECT_TIMEOUT)
            .build()
            .map(|client| Config {
                address,
                datacenter: None,
                http_client: client,
                token: None,
                wait_time: Some(CONNECT_TIMEOUT),
            })
            .map_err(|e| ReadySetError::Internal(e.to_string()))?;

        let consul = consul::Client::new(config);

        let authority = Self {
            consul,
            deployment,
            inner,
        };

        Ok(authority)
    }

    /// Create a new instance.
    pub fn new(connect_string: &str) -> ReadySetResult<Self> {
        let inner = Some(RwLock::new(ConsulAuthorityInner {
            controller_index: None,
            session: None,
        }));
        Self::new_with_inner(connect_string, inner)
    }

    async fn create_session(&self) -> Result<(), Error> {
        let session = {
            let inner = self.read_inner()?;
            inner.session.clone()
        };

        if session.is_none() {
            match self
                .consul
                .create(
                    &SessionEntry {
                        // All keys attached to this session are ephemeral keys.
                        Behavior: Some(SESSION_RELEASE_BEHAVIOR.to_string()),
                        // Disable lock delaying so a new leader can claim the lock
                        // immediately when it is relinquished.
                        LockDelay: Some(SESSION_LOCK_DELAY),
                        // The amount of time to wait for a heartbeat before declaring
                        // a ession dead.
                        TTL: Some(SESSION_TTL.to_string()),
                        ..Default::default()
                    },
                    None,
                )
                .await
            {
                Ok((entry, _)) => {
                    let mut inner = self.write_inner()?;
                    inner.session = Some(entry);
                }
                Err(e) => bail!(e.to_string()),
            }
        };

        Ok(())
    }

    fn get_session(&self) -> Result<String, Error> {
        let inner = self.read_inner()?;
        // Both fields are guarenteed to be populated previously or
        // above.
        #[allow(clippy::unwrap_used)]
        Ok(inner.session.as_ref().unwrap().ID.clone().unwrap())
    }

    fn read_inner(&self) -> Result<RwLockReadGuard<'_, ConsulAuthorityInner>, Error> {
        if let Some(inner_mutex) = &self.inner {
            match inner_mutex.read() {
                Ok(inner) => Ok(inner),
                Err(e) => bail!(internal_err(format!("rwlock is poisoned: '{}'", e))),
            }
        } else {
            bail!(internal_err(
                "attempting to read inner on readonly consul authority"
            ))
        }
    }

    fn write_inner(&self) -> Result<RwLockWriteGuard<'_, ConsulAuthorityInner>, Error> {
        if let Some(inner_mutex) = &self.inner {
            match inner_mutex.write() {
                Ok(inner) => Ok(inner),
                Err(e) => bail!(internal_err(format!("rwlock is poisoned: '{}'", e))),
            }
        } else {
            bail!(internal_err(
                "attempting to mutate inner on readonly consul authority"
            ))
        }
    }

    fn update_controller_index_from_pair(&self, kv: &KVPair) -> Result<(), Error> {
        let mut inner = self.write_inner()?;

        let new_index = match (kv.ModifyIndex, kv.CreateIndex) {
            (Some(modify), _) => modify,
            (_, Some(create)) => create,
            _ => 0,
        };

        inner.controller_index = Some(new_index);
        Ok(())
    }

    fn prefix_with_deployment(&self, path: &str) -> String {
        format!("{}/{}", &self.deployment, path)
    }

    #[cfg(test)]
    async fn destroy_session(&self) -> Result<(), Error> {
        let inner_session = self.read_inner()?.session.clone();
        if let Some(session) = &inner_session {
            // This will not be populated without an id.
            #[allow(clippy::unwrap_used)]
            let id = session.ID.clone().unwrap();
            drop(session);
            self.consul.destroy(&id, None).await.unwrap();
        }

        Ok(())
    }

    #[cfg(test)]
    async fn delete_all_keys(&self) {
        self.consul
            .delete(&self.prefix_with_deployment("?recurse"), None)
            .await
            .unwrap();
    }
}

fn is_new_index(current_index: Option<u64>, kv: &KVPair) -> bool {
    if let Some(current) = current_index {
        match (kv.ModifyIndex, kv.CreateIndex) {
            (Some(modify), _) => modify > current,
            (_, Some(create)) => create > current,
            _ => true,
        }
    } else {
        true
    }
}

#[async_trait]
impl AuthorityControl for ConsulAuthority {
    async fn init(&self) -> Result<(), Error> {
        self.create_session().await
    }

    async fn become_leader(&self, payload: LeaderPayload) -> Result<Option<LeaderPayload>, Error> {
        // Move session creation to the start of the authority.
        let session = self.get_session()?;

        let key = self.prefix_with_deployment(CONTROLLER_KEY);
        let pair = KVPair {
            Key: key.clone(),
            Value: serde_json::to_vec(&payload)?,
            Session: Some(session.clone()),
            ..Default::default()
        };

        // Acquire will only write a new Value for the KVPair if no other leader
        // holds the lock. The lock is released if a leader's session dies.
        match self.consul.acquire(pair, None).await {
            Ok((true, _)) => {
                // Perform a get to update this authorities internal index.
                if let Ok((Some(kv), _)) = self.consul.get(&key, None).await {
                    if kv.Session == Some(session) {
                        self.update_controller_index_from_pair(&kv)?;
                    }
                }

                Ok(Some(payload))
            }
            Ok((false, _)) => Ok(None),
            Err(e) => Err(anyhow!("become_leader consul error: {}", e.to_string())),
        }
    }

    async fn surrender_leadership(&self) -> Result<(), Error> {
        let session = self.get_session()?;

        let pair = KVPair {
            Key: self.prefix_with_deployment(CONTROLLER_KEY),
            Session: Some(session),
            ..Default::default()
        };

        // If we currently hold the lock on CONTROLLER_KEY, we will relinquish it.
        match self.consul.release(pair, None).await {
            Ok(_) => Ok(()),
            Err(e) => bail!(e.to_string()),
        }
    }

    // Block until there is any leader.
    async fn get_leader(&self) -> Result<LeaderPayload, Error> {
        loop {
            match self
                .consul
                .get(&self.prefix_with_deployment(CONTROLLER_KEY), None)
                .await
            {
                Ok((Some(kv), _)) if kv.Session.is_some() => {
                    let bytes = base64::decode(kv.Value)?;
                    return Ok(serde_json::from_slice(&bytes)?);
                }
                _ => tokio::time::sleep(Duration::from_millis(100)).await,
            };
        }
    }

    async fn try_get_leader(&self) -> Result<GetLeaderResult, Error> {
        // Scope `inner` to this block as it is not Send and cannot be held
        // when we hit an await.
        let current_index = {
            let inner = self.read_inner()?;
            inner.controller_index
        };

        Ok(
            match self
                .consul
                .get(&self.prefix_with_deployment(CONTROLLER_KEY), None)
                .await
            {
                Ok((Some(kv), _)) if is_new_index(current_index, &kv) => {
                    // The leader may have changed but if no session holds the lock
                    // then that leader is dead.
                    if kv.Session.is_none() {
                        return Ok(GetLeaderResult::NoLeader);
                    }

                    self.update_controller_index_from_pair(&kv)?;
                    // Consul encodes all responses as base64. Using ?raw to get the
                    // raw value back breaks the client we are using.
                    let bytes = base64::decode(kv.Value)?;
                    GetLeaderResult::NewLeader(serde_json::from_slice(&bytes)?)
                }
                Ok((Some(_), _)) => GetLeaderResult::Unchanged,
                _ => GetLeaderResult::NoLeader,
            },
        )
    }

    fn can_watch(&self) -> bool {
        false
    }

    async fn watch_leader(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn watch_workers(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn try_read<P: DeserializeOwned>(&self, path: &str) -> Result<Option<P>, Error> {
        Ok(
            match self
                .consul
                .get(&self.prefix_with_deployment(path), None)
                .await
            {
                Ok((Some(kv), _)) => {
                    // Consul encodes all responses as base64. Using ?raw to get the
                    // raw value back breaks the client we are using.
                    let bytes = base64::decode(kv.Value)?;
                    Some(serde_json::from_slice(&bytes)?)
                }
                Ok((None, _)) => None,
                // The API currently throws an error that it cannot parse the json
                // if the key does not exist.
                Err(e) => {
                    warn!("try_read consul error: {}", e.to_string());
                    None
                }
            },
        )
    }

    async fn read_modify_write<F, P, E>(&self, path: &str, mut f: F) -> Result<Result<P, E>, Error>
    where
        F: Send + FnMut(Option<P>) -> Result<P, E>,
        P: Send + Serialize + DeserializeOwned,
        E: Send,
    {
        loop {
            // TODO(justin): Use cas parameter to only modify if we have the same
            // ModifyIndex when we write.
            let current_val = self.try_read(path).await?;
            let modified = f(current_val);

            if let Ok(r) = modified {
                let new_val = serde_json::to_vec(&r)?;
                // TODO(justin): Write wrapper.
                let pair = KVPair {
                    Key: self.prefix_with_deployment(path),
                    Value: new_val,
                    ..Default::default()
                };

                match self.consul.put(pair, None).await {
                    Ok((true, _)) => return Ok(Ok(r)),
                    Ok((false, _)) => continue,
                    Err(e) => bail!(e.to_string()),
                }
            }
        }
    }

    /// Updates the controller state only if we are the leader. This is guaranteed by holding a
    /// session that locks both the leader key and the state key. If the leader session dies
    /// both locks will be released.
    async fn update_controller_state<F, P, E>(&self, mut f: F) -> Result<Result<P, E>, Error>
    where
        F: Send + FnMut(Option<P>) -> Result<P, E>,
        P: Send + Serialize + DeserializeOwned,
        E: Send,
    {
        let my_session = Some(self.get_session()?);
        if let Ok((Some(kv), _)) = self
            .consul
            .get(&self.prefix_with_deployment(CONTROLLER_KEY), None)
            .await
        {
            if kv.Session != my_session {
                bail!("Cannot update controller state if not the leader");
            }
        }

        loop {
            // TODO(justin): Use cas parameter to only modify if we have the same
            // ModifyIndex when we write.
            let current_val = self.try_read(STATE_KEY).await?;
            let modified = f(current_val);

            if let Ok(r) = modified {
                let new_val = serde_json::to_vec(&r)?;
                let pair = KVPair {
                    Key: self.prefix_with_deployment(STATE_KEY),
                    Value: new_val,
                    Session: my_session.clone(),
                    ..Default::default()
                };

                match self.consul.acquire(pair, None).await {
                    Ok((true, _)) => return Ok(Ok(r)),
                    Ok((false, _)) => continue,
                    Err(e) => bail!(e.to_string()),
                }
            }
        }
    }

    async fn try_read_raw(&self, path: &str) -> Result<Option<Vec<u8>>, Error> {
        Ok(
            match self
                .consul
                .get(&self.prefix_with_deployment(path), None)
                .await
            {
                Ok((Some(kv), _)) => {
                    // Consul encodes all responses as base64.
                    let bytes = base64::decode(kv.Value)?;
                    Some(serde_json::from_slice::<Vec<u8>>(&bytes)?)
                }
                Ok((None, _)) => None,
                Err(e) => bail!(e.to_string()),
            },
        )
    }

    async fn register_worker(&self, payload: WorkerDescriptor) -> Result<Option<WorkerId>, Error>
    where
        WorkerDescriptor: Serialize,
    {
        // Each worker is associated with the key:
        // `WORKER_PREFIX`/<session>.
        let session = self.get_session()?;
        let key = worker_id_to_path(&session);

        let pair = KVPair {
            Key: self.prefix_with_deployment(&key),
            Value: serde_json::to_vec(&payload)?,
            Session: Some(session.clone()),
            ..Default::default()
        };

        // Acquire will only write a new Value for the KVPair if no other leader
        // holds the lock. The lock is released if a leader's session dies.
        match self.consul.acquire(pair, None).await {
            Ok(_) => Ok(Some(session)),
            Err(e) => bail!(e.to_string()),
        }
    }

    async fn worker_heartbeat(
        &self,
        id: WorkerId,
    ) -> Result<AuthorityWorkerHeartbeatResponse, Error> {
        //TODO(justin): Consider changing this to heartbeat without a parameter.
        Ok(match self.consul.renew(&id, None).await {
            Ok(_) => AuthorityWorkerHeartbeatResponse::Alive,
            Err(e) => {
                error!("Authority failed to heartbeat: {}", e.to_string());
                AuthorityWorkerHeartbeatResponse::Failed
            }
        })
    }

    // TODO(justin): The set of workers includes failed workers, this set will grow
    // unbounded over a long-lived deployment with many failures. Introduce cleanup by
    // deleting keys without a session.
    async fn get_workers(&self) -> Result<HashSet<WorkerId>, Error> {
        Ok(
            match consul::kv::KV::list(
                &self.consul,
                &self.prefix_with_deployment(WORKER_PREFIX),
                None,
            )
            .await
            {
                Ok((children, _)) => children
                    .into_iter()
                    .filter_map(|kv| {
                        if kv.Session.is_some() {
                            Some(path_to_worker_id(&kv.Key))
                        } else {
                            None
                        }
                    })
                    .collect(),
                // The API currently throws an error that it cannot parse the json
                // if the key does not exist.
                Err(e) => bail!(e.to_string()),
            },
        )
    }

    async fn worker_data(
        &self,
        worker_ids: Vec<WorkerId>,
    ) -> Result<HashMap<WorkerId, WorkerDescriptor>, Error> {
        let mut worker_descriptors: HashMap<WorkerId, WorkerDescriptor> = HashMap::new();

        for w in worker_ids {
            if let Ok((Some(kv), _)) = self
                .consul
                .get(&self.prefix_with_deployment(&worker_id_to_path(&w)), None)
                .await
            {
                // Consul encodes all responses as base64. Using ?raw to get the
                // raw value back breaks the client we are using.
                let bytes = base64::decode(kv.Value)?;
                worker_descriptors.insert(w, serde_json::from_slice(&bytes)?);
            }
        }

        Ok(worker_descriptors)
    }

    async fn register_adapter(&self, endpoint: SocketAddr) -> Result<Option<AdapterId>, Error> {
        // Each adapter is associated with the key:
        // `ADAPTER_PREFIX`/<session>.
        let session = self.get_session()?;
        let key = adapter_id_to_path(&session);

        let pair = KVPair {
            Key: self.prefix_with_deployment(&key),
            Value: serde_json::to_vec(&endpoint)?,
            Session: Some(session.clone()),
            ..Default::default()
        };

        // Acquire will only write a new Value for the KVPair if no other leader
        // holds the lock. The lock is released if a leader's session dies.
        match self.consul.acquire(pair, None).await {
            Ok(_) => Ok(Some(session)),
            Err(e) => bail!(e.to_string()),
        }
    }

    async fn get_adapters(&self) -> Result<HashSet<SocketAddr>, Error> {
        let adapter_ids: HashSet<AdapterId> = match consul::kv::KV::list(
            &self.consul,
            &self.prefix_with_deployment(ADAPTER_PREFIX),
            None,
        )
        .await
        {
            Ok((children, _)) => children
                .into_iter()
                .filter_map(|kv| {
                    if kv.Session.is_some() {
                        Some(path_to_adapter_id(&kv.Key))
                    } else {
                        None
                    }
                })
                .collect(),
            // The API currently throws an error that it cannot parse the json
            // if the key does not exist.
            Err(e) => bail!(e.to_string()),
        };

        let consul = self.consul.clone();
        let consul_addresses: Vec<String> = adapter_ids
            .iter()
            .map(|id| self.prefix_with_deployment(&adapter_id_to_path(id)))
            .collect();
        let adapter_futs = consul_addresses
            .iter()
            .map(|addr| consul::kv::KV::get(&consul, addr, None));
        let endpoints = join_all(adapter_futs)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow!(e.to_string()))?
            .into_iter()
            .filter_map(|data| data.0.map(|kv| value_to_socket_addr(kv.Value)))
            .collect::<Result<HashSet<SocketAddr>, Error>>()?;

        Ok(endpoints)
    }
}

/// Helper method to convert a consul value into a SocketAddr.
fn value_to_socket_addr(value: Vec<u8>) -> Result<SocketAddr, Error> {
    let bytes = base64::decode(value)?;
    Ok(serde_json::from_slice::<SocketAddr>(&bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::Url;
    use serial_test::serial;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    #[serial]
    #[ignore]
    async fn read_write_operations() {
        let authority =
            Arc::new(ConsulAuthority::new("http://127.0.0.1:8500/read_write_operations").unwrap());
        authority.init().await.unwrap();
        authority.delete_all_keys().await;

        assert!(authority.try_read::<Duration>("a").await.unwrap().is_none());
        assert_eq!(
            authority
                .read_modify_write("a", |_: Option<Duration>| -> Result<Duration, Duration> {
                    Ok(Duration::from_secs(10))
                })
                .await
                .unwrap(),
            Ok(Duration::from_secs(10))
        );
        assert_eq!(
            authority.try_read::<Duration>("a").await.unwrap(),
            Some(Duration::from_secs(10))
        );
    }

    #[tokio::test]
    #[serial]
    #[ignore]
    async fn leader_election_operations() {
        let authority_address = "http://127.0.0.1:8500/leader_election";
        let authority = Arc::new(ConsulAuthority::new(authority_address).unwrap());
        authority.init().await.unwrap();
        authority.delete_all_keys().await;

        let payload = LeaderPayload {
            controller_uri: url::Url::parse("http://127.0.0.1:2181").unwrap(),
            nonce: 1,
        };
        let expected_leader_payload = payload.clone();
        assert_eq!(
            authority.become_leader(payload.clone()).await.unwrap(),
            Some(payload)
        );
        assert_eq!(
            &authority.get_leader().await.unwrap(),
            &expected_leader_payload
        );

        // This authority can't become the leader because it doesn't hold the lock on the
        // leader key.
        let authority_2 = Arc::new(ConsulAuthority::new(authority_address).unwrap());
        authority_2.init().await.unwrap();
        let payload_2 = LeaderPayload {
            controller_uri: url::Url::parse("http://127.0.0.1:2182").unwrap(),
            nonce: 2,
        };
        // Attempt to become leader, but fail as the other leader still lives.
        authority_2.become_leader(payload_2.clone()).await.unwrap();
        assert_eq!(
            &authority.get_leader().await.unwrap(),
            &expected_leader_payload
        );

        // Regicide.
        authority.destroy_session().await.unwrap();

        // Since the previous leader has died, we should be able to now become the leader.
        authority_2.become_leader(payload_2.clone()).await.unwrap();
        assert_eq!(&authority_2.get_leader().await.unwrap(), &payload_2);

        // Surrender leadership willingly but keep the session alive.
        authority_2.surrender_leadership().await.unwrap();

        let authority_3 = Arc::new(ConsulAuthority::new(authority_address).unwrap());
        authority_3.init().await.unwrap();
        let payload_3 = LeaderPayload {
            controller_uri: url::Url::parse("http://127.0.0.1:2183").unwrap(),
            nonce: 3,
        };
        authority_3.become_leader(payload_3.clone()).await.unwrap();
        assert_eq!(&authority_3.get_leader().await.unwrap(), &payload_3);
    }

    #[tokio::test]
    #[serial]
    #[ignore]
    async fn retrieve_workers() {
        let authority_address = "http://127.0.0.1:8500/retrieve_workers";
        let authority = Arc::new(ConsulAuthority::new(authority_address).unwrap());
        authority.init().await.unwrap();
        authority.delete_all_keys().await;

        let worker = WorkerDescriptor {
            worker_uri: Url::parse("http://127.0.0.1").unwrap(),
            reader_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 1234),
            region: None,
            reader_only: false,
            volume_id: None,
        };

        let workers = authority.get_workers().await.unwrap();
        assert!(workers.is_empty());

        let worker_id = authority
            .register_worker(worker.clone())
            .await
            .unwrap()
            .unwrap();
        let workers = authority.get_workers().await.unwrap();

        assert_eq!(workers.len(), 1);
        assert!(workers.contains(&worker_id));
        assert_eq!(
            authority.worker_heartbeat(worker_id.clone()).await.unwrap(),
            AuthorityWorkerHeartbeatResponse::Alive
        );
        assert_eq!(
            worker,
            authority
                .worker_data(vec![worker_id.clone()])
                .await
                .unwrap()[&worker_id]
        );

        let authority_2 = Arc::new(ConsulAuthority::new(authority_address).unwrap());
        authority_2.init().await.unwrap();
        let worker_id = authority_2.register_worker(worker).await.unwrap().unwrap();
        let workers = authority_2.get_workers().await.unwrap();
        assert_eq!(workers.len(), 2);
        assert!(workers.contains(&worker_id));

        // Kill the session, this should remove the keys from the worker set.
        authority.destroy_session().await.unwrap();
        authority_2.destroy_session().await.unwrap();

        let authority = Arc::new(ConsulAuthority::new(authority_address).unwrap());
        let workers = authority.get_workers().await.unwrap();
        assert_eq!(workers.len(), 0);
    }

    #[tokio::test]
    #[serial]
    #[ignore]
    async fn leader_indexes() {
        let authority_address = "http://127.0.0.1:8500/leader_indexes";
        let authority = Arc::new(ConsulAuthority::new(authority_address).unwrap());
        authority.init().await.unwrap();
        authority.delete_all_keys().await;

        let payload = LeaderPayload {
            controller_uri: url::Url::parse("http://127.0.0.1:2181").unwrap(),
            nonce: 1,
        };

        assert_eq!(
            authority.try_get_leader().await.unwrap(),
            GetLeaderResult::NoLeader,
        );

        assert_eq!(
            authority.become_leader(payload.clone()).await.unwrap(),
            Some(payload)
        );
        // This should be unchanged as the index should have been set in
        // `become_leader`.
        assert_eq!(
            authority.try_get_leader().await.unwrap(),
            GetLeaderResult::Unchanged,
        );
        authority.destroy_session().await.unwrap();

        let authority = Arc::new(ConsulAuthority::new(authority_address).unwrap());
        assert_eq!(
            authority.try_get_leader().await.unwrap(),
            GetLeaderResult::NoLeader,
        );
    }

    #[tokio::test]
    #[serial]
    #[ignore]
    async fn retrieve_adapter_endpoints() {
        let authority_address = "http://127.0.0.1:8500/retrieve_adapter_endpoints";
        let authority = Arc::new(ConsulAuthority::new(authority_address).unwrap());
        authority.init().await.unwrap();
        authority.delete_all_keys().await;

        let adapter_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 1234);

        let adapter_addrs = authority.get_adapters().await.unwrap();
        assert!(adapter_addrs.is_empty());

        authority
            .register_adapter(adapter_addr)
            .await
            .unwrap()
            .unwrap();
        let adapter_addrs = authority.get_adapters().await.unwrap();

        assert_eq!(adapter_addrs.len(), 1);
        assert!(adapter_addrs.contains(&adapter_addr));

        // Kill the session, this should remove the keys from the worker set.
        authority.destroy_session().await.unwrap();

        let authority = Arc::new(ConsulAuthority::new(authority_address).unwrap());
        let adapter_addrs = authority.get_adapters().await.unwrap();
        assert_eq!(adapter_addrs.len(), 0);
    }

    #[tokio::test]
    #[serial]
    #[ignore]
    async fn only_leader_can_update_state() {
        let authority_address = "http://127.0.0.1:8500/leader_updates_state";
        let authority = Arc::new(ConsulAuthority::new(authority_address).unwrap());
        authority.init().await.unwrap();
        authority.delete_all_keys().await;

        let payload = LeaderPayload {
            controller_uri: url::Url::parse("http://127.0.0.1:2181").unwrap(),
            nonce: 1,
        };

        assert_eq!(
            authority.become_leader(payload.clone()).await.unwrap(),
            Some(payload)
        );

        assert_eq!(
            0,
            authority
                .update_controller_state(|n: Option<u32>| -> Result<u32, ()> {
                    match n {
                        None => Ok(0),
                        Some(mut n) => Ok({
                            n += 1;
                            n
                        }),
                    }
                })
                .await
                .unwrap()
                .unwrap()
        );

        let authority_new = Arc::new(ConsulAuthority::new(authority_address).unwrap());
        authority_new.init().await.unwrap();

        assert!(authority_new
            .update_controller_state(|n: Option<u32>| -> Result<u32, ()> {
                match n {
                    None => Ok(40),
                    Some(mut n) => Ok({
                        n += 1;
                        n
                    }),
                }
            })
            .await
            .is_err());

        authority.destroy_session().await.unwrap();

        let payload = LeaderPayload {
            controller_uri: url::Url::parse("http://127.0.0.1:2181").unwrap(),
            nonce: 2,
        };

        assert_eq!(
            authority_new.become_leader(payload.clone()).await.unwrap(),
            Some(payload)
        );

        assert_eq!(
            1,
            authority_new
                .update_controller_state(|n: Option<u32>| -> Result<u32, ()> {
                    match n {
                        None => Ok(0),
                        Some(mut n) => Ok({
                            n += 1;
                            n
                        }),
                    }
                })
                .await
                .unwrap()
                .unwrap()
        );
    }
}
