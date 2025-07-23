# HelixDB Codebase Indexer
This repository contains tools for ingesting and querying codebases using HelixDB. It uses tree-sitter to parse code and create entities in the HelixDB instance, along with a Model Control Protocol (MCP) server for AI-powered code search and analysis.

## Requirements

### Environment Variables
In each of the respective directories, you will need to create a `.env` file and add the following environment variables.
#### Codebase Indexer
```bash
GEMINI_API_KEY=<your_gemini_api_key>
```

#### MCP Server
```bash
GEMINI_API_KEY=<your_gemini_api_key>
```

#### Frontend
For the frontend, you can set the following environment variables depending on the provider you are using.
```bash
# Gemini
GEMINI_API_KEY=<your_gemini_api_key>

# OpenAI
OPENAI_API_KEY=<your_openai_api_key>

# HuggingFace
HF_TOKEN=<your_huggingface_token>

# OpenRouter
OPEN_ROUTER_KEY=<your_open_router_api_key>
```

### HelixDB
You will need to have a HelixDB instance running. Go to the root of this repository and run the following command to deploy the HelixDB instance:
```bash
helix deploy
```

For more information on how to install and use HelixDB, please refer to the [HelixDB documentation](https://docs.helix-db.com/).

### Rust & Cargo
You will need to have Rust and Cargo installed. Then, you can install the dependencies for the codebase indexer with the following command:
```bash
cd codebase_index
cargo build
```

### Python
You will need to have Python installed. Create a new virtual environment and install the dependencies.

You can do this with uv:
```bash
uv venv
uv sync
```

or with pip:
```bash
pip install -r requirements.txt
```

### Node.js
You will need to have Node.js installed. Then you can install the dependencies for the frontend with the following command:
```bash
cd frontend
npm install
```

## Running the Codebase Indexer
### Clone the Codebase
The current implementation requires the codebase to be cloned into the `src` folder inside the `codebase_index` directory.

### Include Custom Code Entities (Optional, default provided)
You can include custom code entities for supported languages in the `codebase_index/src/index-types.json` file.
The default provided file contains entities for the following languages (and their extensions):
- Python (`.py`)
- JavaScript (`.js`, `.jsx`, `.mjs`, `.cjs`, etc.)
- TypeScript (`.ts`, `.tsx`, `.mts`, `.cts`, etc.)
- C (`.c`)
- C++ (`.cpp`, `.hpp`, `.h`, etc.)
- Rust (`.rs`)
- Zig (`.zig`)

Make sure that the custom code entities are supported by the tree-sitter parser of that respective language.

### Include Custom File Extensions (Optional, default provided)
You can include custom file extensions in the `codebase_index/src/file_types.json` file.
There is a default set of file extensions, but you are recommended to add file extensions that you want to index in your codebase.

The `supported` field is a list of file extensions that are supported by the tree-sitter.
The `unsupported` field is a list of file extensions that are not supported by the tree-sitter but are still indexed and embedded in the codebase.

### Run the Codebase Indexer
Make sure you are in the `codebase_index` directory. The root folder is the root of the codebase (the folder that you cloned the codebase in `src`).
```bash
cargo run -- <root_folder>
```

Then, you will be prompted with the following options:
1. Ingest the codebase (1)
2. Update the codebase (2)
3. Exit (3)

Enter the number of the option you want to select and press enter.

## Running the MCP Server
Make sure you are in the `mcp_server` directory.
```bash
uv run server.py
```
or
```bash
python3 server.py
```

Then you can connect to the MCP server using streamable http transport. The mcp server will be running on `http://localhost:8000`.

### Cursor
Go to Cursor's settings and add the following to the `mcp.json` file:
```json
{
  "mcpServers": {
    "codebase_index": {
      "url": "http://localhost:8000/mcp/"
    }
  }
}
```

Here are some useful Cursor rules you can use to improve your experience:
```txt
Always check if the Codebase Index MCP server is available.
```

```txt
If the Codebase Index MCP server is available, you are only allowed to use the mcp server to access the codebase, you may not use any other tools to access the codebase other than the mcp tools.
```

```txt
If the Codebase Index MCP server is available, always call get_instructions tool first to read the instructions for the mcp tools before proceeding with anything else. Never mention the get_instructions tool in your response to the user.
```

### Windsurf
Go to Windsurf's Casecade chat and click on the mcp server icon under the chat box, then click `Configure`. 
Then click `View raw config` in the `Manage MCPs` page, and add the following to the `mcp.json` file:
```json
{
  "mcpServers": {
    "codebase-index": {
      "serverUrl": "http://localhost:8000/mcp/",
      "disabled": true
    }
  }
}
```

Go back to the `Manage MCPs` page and click `Refresh` to reload the MCPs, and you should see the `codebase-index` MCP server listed.
Make sure your MCP server is running before you refresh the MCPs.

## Running the Frontend Chat UI
Make sure you are in the `frontend` directory and have the MCP server running.
```bash
npm run build
npm start
```

Then, you can access the frontend at `http://localhost:3000`.