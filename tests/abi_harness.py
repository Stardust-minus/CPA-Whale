#!/usr/bin/env python3
import base64
import ctypes
import hashlib
import json
import os
import pathlib
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parents[1]
PLUGIN = pathlib.Path(
    os.environ.get(
        "CPA_WHALE_PLUGIN",
        ROOT / "build-output/cpa-whale-plugin-linux-amd64.so",
    )
)
USAGE = ROOT / "tests/fixtures/usage-codex.json"


class Buffer(ctypes.Structure):
    _fields_ = [("ptr", ctypes.POINTER(ctypes.c_ubyte)), ("len", ctypes.c_size_t)]


HOST_CALL = ctypes.CFUNCTYPE(
    ctypes.c_int,
    ctypes.c_void_p,
    ctypes.c_char_p,
    ctypes.POINTER(ctypes.c_ubyte),
    ctypes.c_size_t,
    ctypes.POINTER(Buffer),
)
HOST_FREE = ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_size_t)
PLUGIN_CALL = ctypes.CFUNCTYPE(
    ctypes.c_int,
    ctypes.c_char_p,
    ctypes.POINTER(ctypes.c_ubyte),
    ctypes.c_size_t,
    ctypes.POINTER(Buffer),
)
PLUGIN_FREE = ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_size_t)
PLUGIN_SHUTDOWN = ctypes.CFUNCTYPE(None)


class HostApi(ctypes.Structure):
    _fields_ = [
        ("abi_version", ctypes.c_uint32),
        ("host_ctx", ctypes.c_void_p),
        ("call", HOST_CALL),
        ("free_buffer", HOST_FREE),
    ]


class PluginApi(ctypes.Structure):
    _fields_ = [
        ("abi_version", ctypes.c_uint32),
        ("call", PLUGIN_CALL),
        ("free_buffer", PLUGIN_FREE),
        ("shutdown", PLUGIN_SHUTDOWN),
    ]


host_buffers = {}


def host_result(value):
    raw = json.dumps({"ok": True, "result": value}, separators=(",", ":")).encode()
    buffer = ctypes.create_string_buffer(raw)
    address = ctypes.addressof(buffer)
    host_buffers[address] = buffer
    return address, len(raw)


@HOST_CALL
def host_call(_ctx, method, request, request_len, response):
    method = method.decode()
    if method == "host.auth.list":
        result = {
            "files": [
                {
                    "auth_index": "auth-example-a",
                    "provider": "codex",
                    "type": "codex",
                    "status": "active",
                    "email": "must-not-leak@example.com",
                    "path": "/secret/auth.json",
                }
            ]
        }
    elif method == "host.http.do":
        raw_request = ctypes.string_at(request, request_len) if request and request_len else b"{}"
        url = json.loads(raw_request).get("url", "")
        if "radar-insights" in url:
            body = (ROOT / "tests/fixtures/radar-intelligence.json").read_bytes()
        elif "reset-events" in url:
            body = (ROOT / "tests/fixtures/divin-reset-events.json").read_bytes()
        else:
            body = json.dumps(
                {
                    "status": {"indicator": "none", "description": "All Systems Operational"},
                    "page": {"updated_at": "2026-08-30T00:00:00Z"},
                }
            ).encode()
        result = {
            "StatusCode": 200,
            "Headers": {"content-type": ["application/json"]},
            "Body": base64.b64encode(body).decode(),
        }
    else:
        result = {}
    address, length = host_result(result)
    response.contents.ptr = ctypes.cast(address, ctypes.POINTER(ctypes.c_ubyte))
    response.contents.len = length
    return 0


@HOST_FREE
def host_free(pointer, _length):
    host_buffers.pop(int(pointer or 0), None)


def call(api, method, payload):
    raw = payload if isinstance(payload, bytes) else json.dumps(payload, separators=(",", ":")).encode()
    request = (ctypes.c_ubyte * len(raw)).from_buffer_copy(raw) if raw else None
    response = Buffer()
    code = api.call(
        method.encode(),
        ctypes.cast(request, ctypes.POINTER(ctypes.c_ubyte)) if request is not None else None,
        len(raw),
        ctypes.byref(response),
    )
    if not response.ptr or not response.len:
        raise RuntimeError(f"{method}: empty response, code={code}")
    output = ctypes.string_at(response.ptr, response.len)
    api.free_buffer(response.ptr, response.len)
    decoded = json.loads(output)
    if not decoded.get("ok"):
        raise RuntimeError(f"{method}: {decoded}")
    return decoded["result"]


def main():
    library = ctypes.CDLL(str(PLUGIN))
    init = library.cliproxy_plugin_init
    init.argtypes = [ctypes.POINTER(HostApi), ctypes.POINTER(PluginApi)]
    init.restype = ctypes.c_int
    host = HostApi(1, None, host_call, host_free)
    plugin = PluginApi()
    assert init(ctypes.byref(host), ctypes.byref(plugin)) == 0
    assert plugin.abi_version == 1

    token = "abi-harness-token"
    token_hash = hashlib.sha256(token.encode()).hexdigest()
    with tempfile.TemporaryDirectory(prefix="cpa-whale-abi-") as directory:
        database = pathlib.Path(directory) / "metrics.db"
        yaml = (
            "config-version: 2\n"
            "enabled: true\n"
            "storage:\n"
            f"  database: {database}\n"
            "  timezone: UTC\n"
            "  queue-capacity: 32\n"
            "api:\n"
            "  read-tokens:\n"
            "    - id: abi-client\n"
            f"      sha256: {token_hash}\n"
            "quota:\n"
            "  adapters:\n"
            "    - adapter: codex-response-headers\n"
            "      providers: [codex]\n"
            "signals:\n"
            "  enabled: true\n"
            "  sources:\n"
            "    - id: official-status\n"
            "      adapter: statuspage-v2\n"
            "      display-name: Official Status\n"
            "      url: https://example.invalid/status\n"
            "      interval-seconds: 300\n"
            "    - id: intelligence\n"
            "      adapter: codex-radar-intelligence\n"
            "      display-name: Intelligence\n"
            "      url: https://example.invalid/radar-insights\n"
            "      interval-seconds: 600\n"
            "    - id: reset\n"
            "      adapter: divin-reset-events\n"
            "      display-name: Reset Events\n"
            "      url: https://example.invalid/reset-events\n"
            "      tool-filter: codex\n"
            "      interval-seconds: 1800\n"
            + (ROOT / "deploy/pricing-gpt-6-astra.example.yaml").read_text(encoding="utf-8")
        ).encode()
        registration = call(
            plugin,
            "plugin.register",
            {"config_yaml": base64.b64encode(yaml).decode(), "schema_version": 4},
        )
        assert registration["capabilities"]["usage_plugin"] is True
        assert registration["capabilities"]["management_api"] is True

        call(plugin, "usage.handle", USAGE.read_bytes())
        call(plugin, "management.register", {})
        time.sleep(0.5)
        capabilities_response = call(
            plugin,
            "management.handle",
            {
                "Method": "GET",
                "Path": "/v0/resource/plugins/cpa-whale/v1/capabilities",
                "Headers": {"Authorization": [f"Bearer {token}"]},
                "Query": {},
                "Body": "",
                "HostCallbackID": "callback-capabilities",
            },
        )
        assert capabilities_response["StatusCode"] == 200
        capabilities = json.loads(base64.b64decode(capabilities_response["Body"]))
        assert capabilities["schema_version"] == 1
        assert capabilities["features"]["intelligence"] is True
        assert capabilities["features"]["service_status"] is True
        assert capabilities["features"]["pricing"] is True
        astra_capabilities = [
            model for model in capabilities["models"] if model["model"] == "gpt-6-astra"
        ]
        assert len(astra_capabilities) == 1
        assert astra_capabilities[0]["display_name"] == "Astra"
        assert astra_capabilities[0]["priced"] is True

        astra_usage = json.loads(USAGE.read_bytes())
        astra_usage["Model"] = "gpt-6-astra"
        astra_usage["Alias"] = "gpt-6-astra"
        call(plugin, "usage.handle", astra_usage)

        response = call(
            plugin,
            "management.handle",
            {
                "Method": "GET",
                "Path": "/v0/resource/plugins/cpa-whale/v1/snapshot",
                "Headers": {"Authorization": [f"Bearer {token}"]},
                "Query": {},
                "Body": "",
                "HostCallbackID": "callback-test",
            },
        )
        assert response["StatusCode"] == 200
        snapshot_raw = base64.b64decode(response["Body"])
        snapshot = json.loads(snapshot_raw)
        assert snapshot["scope"] == "global"
        assert snapshot["today"]["tokens"]["total_tokens"] == 3000
        astra_models = [model for model in snapshot["models"] if model["model"] == "gpt-6-astra"]
        assert len(astra_models) == 1
        assert astra_models[0]["provider"] == "codex"
        assert astra_models[0]["reasoning_effort"] == "xhigh"
        assert astra_models[0]["totals"]["tokens"]["total_tokens"] == 1500
        assert astra_models[0]["totals"]["estimated_usd_micros"] == 15_700
        assert snapshot["accounts"][0]["label"] == "Codex A"
        assert any(
            signal.get("model") == "gpt-5.6-sol"
            and signal.get("reasoning_effort") == "xhigh"
            and signal.get("value") == 100.55
            for signal in snapshot["signals"]
        )
        assert any(signal.get("confidence") == "official" for signal in snapshot["signals"])
        serialized = json.dumps(snapshot)
        assert "must-not-leak" not in serialized
        assert "must-not-be-persisted" not in serialized

    plugin.shutdown()
    print("ABI harness passed")


if __name__ == "__main__":
    main()
