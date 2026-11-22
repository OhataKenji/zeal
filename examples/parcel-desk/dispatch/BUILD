load("@rules_python//python:defs.bzl", "py_binary", "py_library", "py_test")

py_library(
    name = "dispatch_lib",
    srcs = [
        "__init__.py",
        "quote.py",
    ],
    imports = ["."],
    deps = [
        "//api:python",
        "@com_github_grpc_grpc//src/python/grpcio/grpc:grpcio",
    ],
)

py_binary(
    name = "quote",
    srcs = ["quote.py"],
    main = "quote.py",
    imports = [".."],
    deps = [":dispatch_lib"],
)

py_test(
    name = "quote_contract_test",
    srcs = ["quote_contract_test.py"],
    imports = [".."],
    data = ["//rates:rate_server"],
    deps = [":dispatch_lib"],
)
