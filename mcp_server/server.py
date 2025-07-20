from fastmcp import FastMCP
from fastmcp.server.auth import BearerAuthProvider
import os
import helix
import re
import dotenv
from typing import Dict, List, Any
from google import genai

dotenv.load_dotenv(override=True)

#  -- auth setup --
# but no auth for now

# with open('public_key.pem', 'r') as f:
#     public_key = f.read()

# auth = BearerAuthProvider(
#     public_key=public_key,  
#     issuer="helix-mcp-server",
#     audience="helix-mcp-server",
#     algorithm="RS256"
# )

# mcp = FastMCP(name="Helix MCP", auth=auth)
#  -- auth setup --

with open('instructions.txt', 'r') as f:
    instructions = f.read()

mcp = FastMCP(name="Helix Codebase MCP", instructions=instructions)
db = helix.Client(local=True, port=6969, verbose=True)
gemini_client = genai.Client(api_key=os.getenv("GEMINI_API_KEY"))

ALLOWED_ENDPOINTS = {
    "getRoot",
    "getFolderRoot",
    "getFileRoot",
    "getFolder",
    "getFolderByName",
    "getAllFolders",
    "getRootFolders",
    "getSuperFolders",
    "getSubFolders",
    "getFileFolder",
    "getFile",
    "getFileContent",
    "getFileByName",
    "getAllFiles",
    "getRootFiles",
    "getFolderFiles",
    "getFileEntities",
    "getEntityFile",
    "getSubEntities",
    "getSuperEntity"
}

def extract_endpoints_with_types(file_path: str = None) -> Dict[str, Dict[str, type]]:
    if file_path is None:
        # Get the directory where this script is located
        script_dir = os.path.dirname(os.path.abspath(__file__))
        # Go up one level to the project root, then into helixdb-cfg/queries.hx
        file_path = os.path.join(script_dir, '..', 'helixdb-cfg', 'queries.hx')
    
    type_map = {
        'String': str,
        'ID': str, 
        'I64': int,
        'F64': float,
        '[F64]': List[float],
        '[I64]': List[int],
        '[String]': List[str],
    }
    
    endpoints = {}
    
    with open(file_path, 'r') as file:
        content = file.read()
    
    matches = re.findall(r'QUERY\s+(\w+)\s*\((.*?)\)\s*=>', content, re.DOTALL)
    
    for func_name, params_str in matches:
        param_types = {}
        if params_str.strip():
            params = re.findall(r'(\w+):\s*(\[?\w+\]?)', params_str)
            for param_name, type_name in params:
                param_types[param_name] = type_map.get(type_name, Any)
        
        endpoints[func_name] = param_types
    
    return endpoints

# currently stored in memory
endpoints_with_types = extract_endpoints_with_types()

@mcp.tool
def do_query(endpoint: str, payload: Dict[str, Any]) -> List[Any]:
    """
    Perform a query on the codebase.
    Allowed endpoints: [
    "getRoot",
    "getFolderRoot",
    "getFileRoot",
    "getFolder",
    "getFolderByName",
    "getAllFolders",
    "getRootFolders",
    "getSuperFolders",
    "getSubFolders",
    "getFileFolder",
    "getFile",
    "getFileContent",
    "getFileByName",
    "getAllFiles",
    "getRootFiles",
    "getFolderFiles",
    "getFileEntities",
    "getEntityFile",
    "getSubEntities",
    "getSuperEntity"
    ]

    Args:
        endpoint (str): The endpoint to query.
        payload (Dict[str, Any]): The payload to pass to the endpoint.


    
    """
    # Check if endpoint is allowed
    if endpoint not in ALLOWED_ENDPOINTS:
        raise ValueError(f"Endpoint '{endpoint}' is not allowed. Permitted endpoints: {', '.join(sorted(ALLOWED_ENDPOINTS))}")
    
    if endpoint not in endpoints_with_types:
        raise ValueError(f"Unknown endpoint: {endpoint}")
    
    required_params = endpoints_with_types[endpoint]
    
    missing = set(required_params.keys()) - set(payload.keys())
    if missing:
        raise ValueError(f"Missing parameters: {missing}")
    
    for param, expected_type in required_params.items():
        if param in payload:
            value = payload[param]
            
            if hasattr(expected_type, '__origin__') and expected_type.__origin__ is list:
                if not isinstance(value, list):
                    raise ValueError(f"{param} must be a list")
            elif not isinstance(value, expected_type):
                raise ValueError(f"{param} must be {expected_type.__name__}, got {type(value).__name__}")

    print(f'Called `do_query` with endpoint: {endpoint} and payload:\n{payload}')
    return db.query(endpoint, payload)

@mcp.tool
def semantic_search_code(query: str, k: int = 5) -> List[Any]:
    """
    Perform semantic search to find code entities with content similar to the query.
    This combines embedding generation and similarity search in one step.

    Args:
        query (str): The query to search for.
        k (int): The number of results to return.
    """

    result = gemini_client.models.embed_content(
        model="gemini-embedding-001",
        contents=query,
        config=genai.types.EmbedContentConfig(task_type="CODE_RETRIEVAL_QUERY"))
    
    query_vector = result.embeddings[0].values

    print(f'Called `semantic_search_code` with query: {query} and k: {k}')
    return db.query("searchSuperEntity", {"vector": query_vector, "k": k})

@mcp.resource("meta://about")
def about():
    return {
        "name": "Helix Codebase MCP",
        "description": "Provides code search, querying, and semantic embedding over a Helix-indexed codebase"
    }

@mcp.resource("meta://instructions")
def instructions():
    with open('instructions.txt', 'r') as f:
        instructions = f.read()
        return instructions

if __name__ == "__main__":
    PORT = os.getenv("PORT", 8000)
    mcp.run(transport="http", host="0.0.0.0", port=PORT)