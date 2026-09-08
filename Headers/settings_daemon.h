#ifndef TONTOO_SETTINGS_DAEMON_H
#define TONTOO_SETTINGS_DAEMON_H

/* Basis header for the future settings-daemon C API.
 * No exported functions yet. The socket protocol and client library
 * come first, then this header gains connect/get/set helpers.
 */

#define TONTOO_SETTINGS_DAEMON_VERSION "0.1.0"
#define TONTOO_SETTINGS_DEFAULT_SOCKET "/run/tontoo-settings.sock"

#endif
