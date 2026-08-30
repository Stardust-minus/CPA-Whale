#!/usr/bin/env python3
import hashlib
import secrets

raw = secrets.token_urlsafe(32)
print(f"WHALE_READ_TOKEN={raw}")
print(f"WHALE_READ_TOKEN_SHA256={hashlib.sha256(raw.encode()).hexdigest()}")
