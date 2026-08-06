# onemore-agent

Onemore is a reliable, practical coding agent for the terminal. The npm package
installs the `onemore` command and includes native binaries for supported platforms.

## Install

```powershell
npm install --global onemore-agent
onemore -v
```

Supported package targets:

- Windows x64
- Linux x64
- macOS x64
- macOS ARM64

## Run

```powershell
onemore
onemore --once "Hello"
onemore --provider deepseek
```

On first run, Onemore creates a configuration template in the platform data
directory. Configure a provider API key, then run `onemore` again. Set
`ONEMORE_HOME` to keep configuration and sessions in a separate directory.

Project source and development documentation: https://github.com/WEP-56/onemore
