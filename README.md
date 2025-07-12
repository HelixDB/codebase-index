# Codebase Ingestion into HelixDB via Python SDK

This is a codebase ingestion script for HelixDB. It uses tree-sitter to parse python code and create entities in the HelixDB instance.

## Prerequisites

### Python Environment
It is recommended to create a new virtual environment for this repository. After creating a virtual environment, you can install the required packages:
```bash
uv sync
```

or pip to install the dependencies:
```bash
pip install -r requirements.txt
```

### Installing the Helix CLI
```bash
curl -sSL "https://install.helix-db.com" | bash
helix install
```

## Usage
### Starting the Helix instance
```bash
helix deploy
```

### Ingesting a codebase
Ingesting the whole codebase:
```bash
python3 ingestion.py
```

Ingesting a specific part of the codebase:
```bash
python ingestion.py <path_to_codebase>
```