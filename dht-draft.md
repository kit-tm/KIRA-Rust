# Entwurf DHT #

## Grundsätzliches zum Entwurf ## 

- kein Anspruch reines UML zu sein (u.a. Template Typen, Enums anders)
- als Workaround für einen [Mermaid Bug](https://github.com/mermaid-js/mermaid/issues/4578)
  wurden teilweise `≤` bzw. `≥` im Entwurf als identische Bedeutung für Typ-Parameter verwendet

## Neue Protokollnachrichten und Erweiterung der Domain ##

```mermaid
classDiagram
    namespace dht_messaging {
        class DHTReqRspMessage {
            + nonce: Nonce
        }
        class StoreReq["StoreReq≤D: Serialize≥"] {
            + handle: NodeId
            + data: D
            + store_duration: Duration
            + replicate: bool
        }
        class StoreRsp {
            status: DHTStoreResult
        }
        class FetchReq["FetchReq"] {
            handle: NodeId
        }
        class FetchRsp["FetchRsp≤D: Deserialize≥"] {
            data: DHTFetchResult~D~
        }
    }
    DHTReqRspMessage <|-- StoreReq
    DHTReqRspMessage <|-- StoreRsp
    DHTReqRspMessage <|-- FetchReq
    DHTReqRspMessage <|-- FetchRsp

    namespace dht_domain {
        class StoreResult
        class StoreOk {
            <<enumeration>>
            Created
            Updated
        }
        class StoreErr {
            <<enumeration>>
            TimeOutErr
            DataTypeErr
        }

        class FetchResult["FetchResult≤D≥"]
        class FetchErr {
            <<enumeration>>
            NotFoundErr
            TimeOut
        }
        
        class DHTData
        class Single
        class Slice
        class List
    }
    class Result~T,E~ { <<interface>> }
    Result <|.. StoreResult: «bind» StoreOk, StoreErr
    StoreResult <.. StoreErr
    StoreResult <.. StoreOk
    Result <|.. FetchResult: «bind» D, FetchErr
    FetchResult <.. FetchErr
    Expiring~C~ <|.. DHTData
    DHTData <|-- Single
    DHTData <|-- Slice
    DHTData <|-- List
    
    namespace messaging {
        class ProtocolMessage { <<enumeration>> }
        class ReqRspMessage {
            + nonce: Nonce
        }
    }
    ProtocolMessage <|-- ReqRspMessage
    ReqRspMessage <|-- DHTReqRspMessage

    namespace domain {
        class NodeId {
            + bytes
        } 
    } 
```
#### Anmerkungen: ####

- Erweiterung `ProtocolMessage` nur durch `DHTReqRspMessage`, damit einheitliches Verhalten in Use-Cases für DHT-Nachrichten erreicht werden kann (z.B. forwarding)

## Neue Use Cases ##

### 1. DistributedHashTable ###
```mermaid
classDiagram
    namespace use_cases {
        class UseCaseState {
            <<interface>>
            is_error() bool
        }

        class UseCase {
            <<interface>>
        }
    }

    namespace dht_use_case {
        class DHTState {
            <<enumeration>>
            Running
            Stopped
            Error
        }
        class DistributedHashTable["DistributedHashTable≤D: Serialize + Deserialize≥"] {
            + handle_event(context: UseCaseContext, event: UseCaseEvent)
            - handle_store_req(context: UseCaseContext, event: UseCaseEvent)
            - handle_fetch_req(context: UseCaseContext, event: UseCaseEvent)
        }
        class DistributedHashTableConfig {
            + collect_interval: Duration
            + send_timeout: Duration
        }
        
        class HashTable["HashTable≤H,D≥"] {
            <<interface>>
            store(handle: H, data: D)
            fetch(handle: H) Some~D~
            delete(handle: H)
        }
        
        class Expiring["Expiring≤C≥"]{
            <<interface>>
            expire()
            expire_with_strategy(strategy: TimeoutStrategy~C~)
        }
        
        class TimeoutStrategy["TimeoutStrategy≤C≥"] {
            <<interface>>
            is_timed_out(context: C, time: Instant) bool
        }
        class ConstTimeoutStrategy["ConstTimeoutStrategy"] {
            + expire_after: Duration
            + is_timed_Out(context: (), time: Instant) bool
            ConstTimeoutStrategy(expireAfter: Duration) bool
        }
        
        class ExpiringHashTable["ExpiringHashTable≤H,D: Expire≤H≥≥"] {
            <<interface>>
        }
    }
    ExpiringHashTable --|> HashTable: «bind» H, D
    ExpiringHashTable --|> Expiring: «bind» H
    TimeoutStrategy <|.. ConstTimeoutStrategy: «bind» ()
    
    DHTReqRspMessage <.. DistributedHashTable
    
    DistributedHashTable --> "1 config" DistributedHashTableConfig
    note for DistributedHashTable "verarbeitet eingehende DHT Anfragen"
    UseCaseState <|.. DHTState
    DistributedHashTableConfig --> "1" ExpiringHashTable : «bind» NodeId, D
    DistributedHashTableConfig --> "1" TimeoutStrategy : «bind» NodeId
    UseCase <|.. DistributedHashTable

    namespace daemon {
        class Node {
        start()
        handle_loop(config)
        }
    }
    Node ..> UseCase
```

#### Anmerkungen: ####

- verantwortlich für das **Reagieren** auf Netzwerknachrichten
- **nicht** verantwortlich für Neu-Publizieren, um Werte am Leben zu halten


---


### 2. Distributed Hash Table Access over REST ###

```mermaid
classDiagram
    namespace use_cases {
        class UseCaseState {
            <<interface>>
            is_error() bool
        }

        class UseCase {
            <<interface>>
        }
    }
    namespace dht_rest_use_case {
        class DHTRestState {
            <<enumeration>>
            Running
            Stopped
            Error
        }
        class DistributedHashTableInjector["DistributedHashTableInjector≤H: Into≤NodeID≥, D: Serialize + Deserialize≥"] {
            + store(context: UseCaseContext, handle: H, data: D, restore: bool) StoreResult
            + fetch(context: UseCaseContext, handle: H) FetchResult
            + handleEvent(context: UseCaseContext, event: UseCaseEvent)
        }
        class DistributedHashTableInjectorConfig {
            + periodicRestore: Duration
        }
        
        class HashMap["HashMap≤NodeId,D≥"]
    }
    DistributedHashTableInjector --> "1 config" DistributedHashTableInjectorConfig
    DistributedHashTableInjectorConfig --> "1 store" HashMap
    UseCase <|.. DistributedHashTableInjector
    UseCaseState <|.. DHTRestState
```

#### Anmerkungen: ####

- Zugriff auf DHT über REST-Schnittstelle
- versendet initiale Store-Request
- hält Werte am Leben, falls gewollt

