load("@com_github_grpc_grpc//bazel:cc_grpc_library.bzl", "cc_grpc_library")
load("@com_github_grpc_grpc//bazel:python_rules.bzl", "py_grpc_library", "py_proto_library")
load("@com_google_protobuf//bazel:cc_proto_library.bzl", "cc_proto_library")
load("@rules_proto//proto:defs.bzl", "proto_library")
load("@rules_python//python:defs.bzl", "py_library")

package(default_visibility = ["//visibility:public"])

proto_library(
    name = "rates_proto",
    srcs = ["rates.proto"],
)

cc_proto_library(
    name = "rates_cc_proto",
    deps = [":rates_proto"],
)

cc_grpc_library(
    name = "rates_cc_grpc",
    srcs = [":rates_proto"],
    grpc_only = True,
    deps = [":rates_cc_proto"],
)

py_proto_library(
    name = "rates_py_pb2",
    deps = [":rates_proto"],
)

py_grpc_library(
    name = "rates_py_pb2_grpc",
    srcs = [":rates_proto"],
    deps = [":rates_py_pb2"],
    grpc_library = "@com_github_grpc_grpc//src/python/grpcio/grpc:grpcio",
)

# Shared Python stubs: `api.rates_pb2` and `api.rates_pb2_grpc`.
py_library(
    name = "python",
    srcs = ["__init__.py"],
    imports = [".."],
    deps = [
        ":rates_py_pb2",
        ":rates_py_pb2_grpc",
    ],
)
