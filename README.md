# Rust-in-pieces
A microservice demo application, forked from [google's microservices-demo](https://github.com/GoogleCloudPlatform/microservices-demo), with several refactored services now written in rust.

## Deploy (Docker Services on Minikube)

1. Ensure you have the following requirements:
   - minikube - [install minikube](minikube.sigs.k8s.io/docs/start/)
   - kubectl - [install kubectl](https://kubernetes.io/docs/tasks/tools/)
   - skaffold 2.0.2+ - [install skaffold](https://skaffold.dev/docs/install/)
   - A node w min. 4 CPUs, 4.0 GiB memory, 32 GB disk space

2. Launch minikube
```
minikube start --cpus=4 --memory 4096 --disk-size 32g
```

3. Run `kubectl get nodes` to verify you're connected to the respective control plane.

4. Run `skaffold run` (first time will be slow, it can take ~20 minutes). This will build and deploy the application. If you need to rebuild the images automatically as you refactor the code, run `skaffold dev` command.

5. Run `kubectl get pods` to verify the Pods are ready and running.

6. Run `kubectl port-forward deployment/frontend 8080:8080` to forward a port to the frontend service.

7. Navigate to `localhost:8080` to access the web frontend.


## Deploy wasm-service 

1. Ensure you have the following requirements:
   - rustup: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
   - wasm32-wasip2: `rustup target add wasm32-wasip2`


2. `src/shippingserwasm/src/main.rs` contains the rust source code for the wasm-ready shipping service. Compile it to a webassembly component via
```
cd ./src/shippingserwasm/
cargo build --lib --target wasm32-wasip2 --release && \
cp target/wasm32-wasip2/release/shippingserwasm.wasm .
```

3. Once compiled, build and run a server that and executes a WebAssembly component using Wasmtime's WASI runtime. The source code for the server is under `src/shippingserwasm/bin/serve.rs`
```
cargo build --bin serve
```
```
cargo run --bin serve
```

4. Verify it works with a simple grpc client. We've included one under `src/shippingservice/src/client.rs`. Open a new Terminal window and run
```
cd src/shippingservice
cargo run --bin shipping-client
```

## Architecture

**Online Boutique** is composed of 11 microservices written in different
languages that talk to each other over gRPC.

[![Architecture of
microservices](/docs/img/architecture-diagram.png)](/docs/img/architecture-diagram.png)

Find **Protocol Buffers Descriptions** at the [`./protos` directory](/protos).

| Service                                              | Language      | Description                                                                                                                       |
| ---------------------------------------------------- | ------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| [frontend](/src/frontend)                           | Go            | Exposes an HTTP server to serve the website. Does not require signup/login and generates session IDs for all users automatically. |
| [cartservice](/src/cartservice)                     | C#            | Stores the items in the user's shopping cart in Redis and retrieves it.                                                           |
| [productcatalogservice](/src/productcatalogservice) | Go            | Provides the list of products from a JSON file and ability to search products and get individual products.                        |
| [currencyservice](/src/currencyservice)             | Node.js       | Converts one money amount to another currency. Uses real values fetched from European Central Bank. It's the highest QPS service. |
| [paymentservice](/src/paymentservice)               | Node.js       | Charges the given credit card info (mock) with the given amount and returns a transaction ID.                                     |
| [shippingservice](/src/shippingservice)             | ~~Go~~ Rust | Gives shipping cost estimates based on the shopping cart. Ships items to the given address (mock)                                 |
| [emailservice](/src/emailservice)                   | Python        | Sends users an order confirmation email (mock).                                                                                   |
| [checkoutservice](/src/checkoutservice)             | Go            | Retrieves user cart, prepares order and orchestrates the payment, shipping and the email notification.                            |
| [recommendationservice](/src/recommendationservice) | Python        | Recommends other products based on what's given in the cart.                                                                      |
| [adservice](/src/adservice)                         | Java          | Provides text ads based on given context words.                                                                                   |
| [loadgenerator](/src/loadgenerator)                 | Python/Locust | Continuously sends requests imitating realistic user shopping flows to the frontend.                                              |

## Screenshots

| Home Page                                                                                                         | Checkout Screen                                                                                                    |
| ----------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ |
| [![Screenshot of store homepage](/docs/img/online-boutique-frontend-1.png)](/docs/img/online-boutique-frontend-1.png) | [![Screenshot of checkout screen](/docs/img/online-boutique-frontend-2.png)](/docs/img/online-boutique-frontend-2.png) |

