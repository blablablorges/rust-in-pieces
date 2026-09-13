FROM golang:1.10-alpine as builder
RUN apk add --no-cache \
  ca-certificates \
  git
# go get -d ./... would fetch HEAD of these deps, which today require a much
# newer Go toolchain than this image ships; pin the revisions this service
# actually shipped against instead (from the era's Gopkg.lock).
RUN mkdir -p /go/src/github.com/golang/protobuf /go/src/golang.org/x/net /go/src/google.golang.org/grpc /go/src/golang.org/x/sys /go/src/golang.org/x/text /go/src/google.golang.org/genproto && \
  git clone -q https://github.com/golang/protobuf /go/src/github.com/golang/protobuf && \
  git -C /go/src/github.com/golang/protobuf checkout -q b4deda0973fb4c70b50d226b1af49f3da59f5265 && \
  git clone -q https://github.com/golang/net /go/src/golang.org/x/net && \
  git -C /go/src/golang.org/x/net checkout -q f4c29de78a2a91c00474a2e689954305c350adf9 && \
  git clone -q https://github.com/grpc/grpc-go /go/src/google.golang.org/grpc && \
  git -C /go/src/google.golang.org/grpc checkout -q 32fb0ac620c32ba40a4626ddf94d90d12cce3455 && \
  git clone -q https://github.com/golang/sys /go/src/golang.org/x/sys && \
  git -C /go/src/golang.org/x/sys checkout -q 0ffbfd41fbef8ffcf9b62b0b0aa3a5873ed7a4fe && \
  git clone -q https://github.com/golang/text /go/src/golang.org/x/text && \
  git -C /go/src/golang.org/x/text checkout -q f21a4dfb5e38f5895301dc265a8def02365cc3d0 && \
  git clone -q https://github.com/googleapis/go-genproto /go/src/google.golang.org/genproto && \
  git -C /go/src/google.golang.org/genproto checkout -q daca94659cb50e9f37c1b834680f2e46358f10b0
WORKDIR /src/microservices-demo/shippingservice
COPY . .
RUN CGO_ENABLED=0 go build -o /shippingservice .

FROM alpine as release
COPY --from=builder /shippingservice /shippingservice
ENV APP_PORT=50051
EXPOSE 50051
ENTRYPOINT ["/shippingservice"]
