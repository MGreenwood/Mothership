# Quick CLI Test Results

## Summary
- Total: 19
- Working: 11
- Failed: 1
- Timeout: 0
- Placeholder: 0

## Command Details
### --help
Status: **WORKING**
Return code: 0
Output preview:
```
Mothership - Frictionless Version Control

Usage: mothership.exe <COMMAND>

Commands:
  auth        Authenticate this machine with Mothership
  gateway     Gateway operations (list, create projects)
 ...
```

### --version
Status: **UNKNOWN**
Return code: 0
Output preview:
```
mothership 0.0.51

```

### auth --help
Status: **WORKING**
Return code: 0
Output preview:
```
Authenticate this machine with Mothership

Usage: mothership.exe auth [COMMAND]

Commands:
  google  Login with Google OAuth
  github  Login with GitHub OAuth
  help    Print this message or the help ...
```

### gateway --help
Status: **WORKING**
Return code: 0
Output preview:
```
Gateway operations (list, create projects)

Usage: mothership.exe gateway <COMMAND>

Commands:
  list        List available projects (default)
  create      Create a new gateway project
  disconnect  ...
```

### gateway list
Status: **UNKNOWN**
Return code: 0

### init --help
Status: **WORKING**
Return code: 0
Output preview:
```
Initialize a new project in current directory

Usage: mothership.exe init [NAME]

Arguments:
  [NAME]  Project name (optional, defaults to directory name)

Options:
  -h, --help  Print help

```

### beam --help
Status: **WORKING**
Return code: 0
Output preview:
```
Beam into a project for development

Usage: mothership.exe beam [OPTIONS] [PROJECT]

Arguments:
  [PROJECT]  Project name or ID to beam into (optional if in project directory) [default: ]

Options:
  ...
```

### status
Status: **UNKNOWN**
Return code: 0

### rift --help
Status: **WORKING**
Return code: 0
Output preview:
```
Rift management operations

Usage: mothership.exe rift <COMMAND>

Commands:
  list    List all rifts in the current project
  new     Create a new rift
  switch  Switch to a different rift
  status  S...
```

### rift list
Status: **FAILED**
Return code: 1

### daemon --help
Status: **WORKING**
Return code: 0
Output preview:
```
Daemon management operations

Usage: mothership.exe daemon <COMMAND>

Commands:
  status   Show daemon status and tracked projects
  stop     Stop the background daemon
  restart  Restart the backgrou...
```

### daemon status
Status: **UNKNOWN**
Return code: 0
Output preview:
```
Checking daemon status...
Mothership daemon is running
Daemon Status:
{"status":"success","data":{"is_running":true,"projects_tracked":0,"files_syncing":0,"last_sync":null,"server_connected":fa...
```

### checkpoint --help
Status: **WORKING**
Return code: 0
Output preview:
```
Create a checkpoint (commit changes)

Usage: mothership.exe checkpoint <MESSAGE>

Arguments:
  <MESSAGE>  Checkpoint message

Options:
  -h, --help  Print help

```

### sync
Status: **UNKNOWN**
Return code: 0
Output preview:
```
Syncing with remote Mothership...

```

### history
Status: **UNKNOWN**
Return code: 0

### server --help
Status: **WORKING**
Return code: 0
Output preview:
```
Disconnect from the current Mothership server (switch to local-only mode)

Usage: mothership.exe server <COMMAND>

Commands:
  status      Show current server connection status
  disconnect  Disconnec...
```

### server status
Status: **UNKNOWN**
Return code: 0
Output preview:
```
Checking server connection status...
Not connected to any server
Operating in local-only mode
Use 'mothership connect <server-url>' to connect to a server

```

### connect --help
Status: **WORKING**
Return code: 0
Output preview:
```
Connect to a Mothership server

Usage: mothership.exe connect <SERVER_URL>

Arguments:
  <SERVER_URL>  Server URL (e.g., https://mothership.company.com)

Options:
  -h, --help  Print help

```

### update --help
Status: **WORKING**
Return code: 0
Output preview:
```
Update the CLI to the latest version

Usage: mothership.exe update [OPTIONS]

Options:
      --check-only         Check for updates without installing
      --force              Force update even if c...
```