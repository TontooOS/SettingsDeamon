#!/usr/bin/env python3
"""Change the TontooOS customization through the settings daemon socket.

Acts like the Settings app (`com.tontoo.systemsettings`): it sends the
private `customize_set` op over the unix socket and prints the effective
settings. Only the keys passed on the command line are changed, everything
else keeps its stored value.

Examples:
  Show the current customization:
    python3 set_customize.py --get

  Switch to light mode with a blue accent:
    python3 set_customize.py --theme light --accent blue

  Change only the accent color:
    python3 set_customize.py --accent red

  Touch the LiquidGlass slider:
    python3 set_customize.py --glass much
"""

import argparse
import json
import os
import socket
import sys

DEFAULT_SOCKET_PATH = "/run/tontoo-settings.sock"
SOCKET_ENV_VAR = "SETTINGS_SOCKET"

ACCENTS = [
    "multicolor",
    "blue",
    "red",
    "orange",
    "yellow",
    "green",
    "teal",
    "cyan",
    "indigo",
    "purple",
    "purple2",
    "pink",
    "gray",
]
THEMES = ["dark", "light"]
GLASS_AMOUNTS = ["much", "glass", "less"]


def send_request(sock_path, op, params):
    """Send one newline-delimited JSON frame, return the parsed reply."""
    request = {"id": 1, "op": op, "params": params}
    line = json.dumps(request) + "\n"
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        client.connect(sock_path)
    except OSError as exc:
        raise SystemExit(
            "cannot reach the settings daemon at {}: {}".format(sock_path, exc)
        )
    with client:
        client.sendall(line.encode("utf-8"))
        reply = b""
        while not reply.endswith(b"\n"):
            chunk = client.recv(4096)
            if not chunk:
                break
            reply += chunk
    try:
        return json.loads(reply.decode("utf-8"))
    except ValueError as exc:
        raise SystemExit("invalid reply from the daemon: {}".format(exc))


def print_settings(result):
    """Print the effective customization in a stable key order."""
    for key in ("wallpaper", "accent", "theme", "glass", "revision"):
        if key in result:
            print("{}: {}".format(key, result[key]))


def main(argv=None):
    parser = argparse.ArgumentParser(
        description="Change accent color, theme and glass via the settings daemon."
    )
    parser.add_argument(
        "--socket",
        default=os.environ.get(SOCKET_ENV_VAR, DEFAULT_SOCKET_PATH),
        help="Daemon socket path (default: %(default)s, env {} wins)".format(
            SOCKET_ENV_VAR
        ),
    )
    parser.add_argument(
        "--get",
        action="store_true",
        help="Show the current customization instead of changing it.",
    )
    parser.add_argument(
        "--accent", choices=ACCENTS, help="Accent color name."
    )
    parser.add_argument("--theme", choices=THEMES, help="Color theme.")
    parser.add_argument(
        "--glass", choices=GLASS_AMOUNTS, help="LiquidGlass amount."
    )
    parser.add_argument(
        "--wallpaper",
        help="Wallpaper pack id (e.g. THAOELAKE). Must not be empty.",
    )
    args = parser.parse_args(argv)

    if args.get:
        reply = send_request(args.socket, "customize_get", {})
    else:
        params = {}
        if args.accent is not None:
            params["accent"] = args.accent
        if args.theme is not None:
            params["theme"] = args.theme
        if args.glass is not None:
            params["glass"] = args.glass
        if args.wallpaper is not None:
            if not args.wallpaper:
                raise SystemExit("wallpaper must not be empty")
            params["wallpaper"] = args.wallpaper
        if not params:
            parser.error("nothing to change: pass --accent, --theme, --glass or --wallpaper (or --get)")
        reply = send_request(args.socket, "customize_set", params)

    if not reply.get("ok", False):
        raise SystemExit("daemon refused the request: {}".format(reply.get("error", "unknown error")))
    print_settings(reply.get("result", {}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
