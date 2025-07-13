from tree_sitter import Language, Parser
import tree_sitter_python, tree_sitter_javascript
import os
import time
import argparse
import pathspec
import hashlib
from concurrent.futures import ThreadPoolExecutor
from helix import Client, Instance

# Default patterns to always ignore
DEFAULT_IGNORE_PATTERNS = ['.git/']

# Parser Languages
PY_LANGUAGE = Language(tree_sitter_python.language())
JS_LANGUAGE = Language(tree_sitter_javascript.language())

# Parsers
py_parser = Parser(PY_LANGUAGE)
js_parser = Parser(JS_LANGUAGE)

# Maximum depth of sub entities to process
MAX_DEPTH = 2

# HelixDB Instance
instance = Instance()
time.sleep(1)

# HelixDB Client
client = Client(local=True, verbose=False)

# Thread pool for parallel processing
MAX_WORKERS = min(os.cpu_count()//2, 8)
executor = ThreadPoolExecutor(max_workers=MAX_WORKERS)

# Cache for seen files to avoid re-parsing
seen_files = set()

# Cache for directory-specific PathSpecs
spec_map = {}

# Ingestion function
def ingestion(root_path):
    # Ensure root_path is absolute
    root_path = os.path.abspath(root_path)

    # Load gitignore specs at the start
    gitignore_specs, root_dir = load_gitignore_specs(root_path)
    
    root_id = client.query('createRoot', {'name': root_path})[0]['root'][0]['id']
    populate(root_path, parent_id=root_id, gitignore_specs=gitignore_specs, root_dir=root_dir)

# Modifiable helper functions
# TODO: Replace with actual chunking function
def chunk_entity(text:str):
    return [text[i:i+1000] for i in range(0, len(text), 1000)]

# TODO: Replace with actual embedding function
def random_embedding(text:str):
    return [0.1 for _ in range(768)]

# Helper functions
def populate(full_path, curr_type='root', parent_id=None, gitignore_specs=None, root_dir=None):
    dir_dict = scan_directory(full_path, gitignore_specs, root_dir)
    
    # Extract gitignore specs and root_dir if they were returned by scan_directory
    gitignore_specs = dir_dict.get("gitignore_specs", gitignore_specs)
    root_dir = dir_dict.get("root_dir", root_dir)

    print(f'\nProcessing {len(dir_dict["folders"])} folders')
    
    # Store futures for parallel folder processing
    folder_futures = []
    
    # First create all folder entries
    for folder in dir_dict["folders"]:
        print(f"\nProcessing folder: {folder}")
        if curr_type == 'root':
            # Create super folder
            folder_id = client.query('createSuperFolder', {'root_id': parent_id, 'name': folder})[0]['folder'][0]['id']
        else:
            # Create sub folder
            folder_id = client.query('createSubFolder', {'folder_id': parent_id, 'name': folder})[0]['subfolder'][0]['id']
        
        # Submit folder processing to thread pool for parallel execution
        folder_path = os.path.join(full_path, folder)
        folder_futures.append(executor.submit(
            populate, 
            folder_path, 
            'folder', 
            folder_id, 
            gitignore_specs, 
            root_dir
        ))

    # Process files in parallel
    print(f'\nProcessing {len(dir_dict["files"])} files')
    
    # Filter out ignored files
    files_to_process = [file for file in dir_dict["files"] if not is_ignored(os.path.join(full_path, file), gitignore_specs, root_dir)]
    
    # Submit file processing tasks to the thread pool
    file_futures = []
    for file in files_to_process:
        print(f"\nSubmitting {file} for processing")
        file_futures.append(executor.submit(
            process_file,
            file,
            full_path,
            curr_type,
            parent_id
        ))
    
    # Wait for all file processing to complete
    for future in file_futures:
        try:
            success = future.result()
        except Exception as e:
            print(f"Error in file processing: {e}")
    
    # Wait for all folder processing to complete
    for future in folder_futures:
        try:
            future.result()
        except Exception as e:
            print(f"Error in folder processing: {e}")
    
    del dir_dict

def process_file(file, full_path, curr_type, parent_id):
    print(f"{file} is from {curr_type}")
    try:
        parser = None
        if file.endswith('.py'):
            parser = py_parser
        # elif file.endswith('.js'):
        #     parser = js_parser
        elif file == '.gitignore':
            parser = None
        
        if parser is not None:
            # Extract python code structure with tree-sitter
            file_path = os.path.join(full_path, file)
            extension = file.split('.')[-1]
            tree, code = parse_file(file_path, parser)

            if tree:
                tree_dict = node_to_dict(tree.root_node, code, 0)
                del tree
                del code

                if curr_type == 'root':
                    # Create super file
                    file_id = client.query('createSuperFile', {'root_id': parent_id, 'name': file, 'extension': extension, 'text': tree_dict['text']})[0]['file'][0]['id']
                else:
                    # Create sub file
                    file_id = client.query('createFile', {'folder_id': parent_id, 'name': file, 'extension': extension, 'text': tree_dict['text']})[0]['file'][0]['id']

                children = tree_dict['children']
                del tree_dict

                print(f"\nProcessing {len(children)} super entities in {file}")
                for superentity in children:
                    # Create super entity
                    super_entity_id = client.query('createSuperEntity', {'file_id': file_id, 'entity_type': superentity['type'], 'start_byte': superentity['start_byte'], 'end_byte': superentity['end_byte'], 'order': superentity['order'], 'text': superentity['text']})[0]['entity'][0]['id']
                    
                    # Embed super entity
                    chunks = chunk_entity(superentity['text'])
                    for chunk in chunks:
                        client.query('embedSuperEntity', {'entity_id':super_entity_id, 'vector': random_embedding(chunk)})
                        del chunk

                    del chunks

                    process_entities(superentity, super_entity_id)
                    
                    del superentity

                del children
                return True
            else:
                print(f'Failed to parse file: {file}')
                if tree is not None:
                    del tree
                if code is not None:
                    del code
                return False
        else:
            print(f'Ignored: {file}')
            return False
    except Exception as e:
        print(f"Error processing file {file}: {e}")
        return False

def process_entities(parent_dict, parent_id, step = 0):
    if step < MAX_DEPTH and 'children' in parent_dict and len(parent_dict['children']) > 0:

        children = parent_dict['children']
        payload = [{'entity_id': parent_id, 'entity_type': entity['type'], 'start_byte': entity['start_byte'], 'end_byte': entity['end_byte'], 'order': entity['order'], 'text': entity['text']} for entity in children]
        
        if len(payload) < 1:
            return
        entity_ids = [entity['entity'][0]['id'] for entity in client.query('createSubEntity', payload)]
        del payload

        for i in range(len(entity_ids)):
            process_entities(children[i], entity_ids[i], step + 1)

        del children
        del entity_ids

def parse_file(file_path, parser):
    try:
        with open(file_path, 'rb') as file:
            source_code = file.read()
            
        # Calculate file hash to avoid re-parsing identical files
        file_hash = hashlib.sha1(source_code).hexdigest()
        if file_hash in seen_files:
            print(f"Ignored duplicate: {file_path}")
            return None, None
            
        seen_files.add(file_hash)
        return parser.parse(source_code), source_code
    except Exception as e:
        print(f"Error parsing {file_path}: {e}")
        return None, None

def node_to_dict(node, source_code, order:int=1):
    return {
        "type": node.type,
        "start_byte": node.start_byte,
        "end_byte": node.end_byte,
        "order": order,
        "text": source_code[node.start_byte:node.end_byte].decode('utf8'),
        "children": [node_to_dict(child, source_code, i+1) for i, child in enumerate(node.children)]
    }

# Cache for PathSpec objects to avoid rebuilding them
_spec_cache = {}

def get_spec(patterns):
    """Get a cached PathSpec object for the given patterns."""
    if not patterns:
        return None
        
    # Create a hash key from the patterns
    key = hashlib.sha1("\n".join(patterns).encode()).hexdigest()
    
    # Return cached spec if available
    if key not in _spec_cache:
        _spec_cache[key] = pathspec.PathSpec.from_lines("gitwildmatch", patterns)
    
    return _spec_cache[key]

def get_spec_for_path(path, gitignore_specs):
    """Get or create a PathSpec for a specific directory path."""
    # Use absolute path as the key
    abs_path = os.path.abspath(path) if not os.path.isabs(path) else path
    
    # Return cached spec if available
    if abs_path in spec_map:
        return spec_map[abs_path]
    
    # Check for a .gitignore file in the current directory
    local_gitignore = os.path.join(abs_path, '.gitignore')
    
    # Start with a copy of the existing specs
    path_specs = dict(gitignore_specs) if gitignore_specs else {}
    
    # Add the local .gitignore if it exists and hasn't been processed
    if os.path.isfile(local_gitignore) and abs_path not in path_specs:
        try:
            with open(local_gitignore, 'r') as f:
                patterns = f.read().splitlines()
                # Filter out empty lines and comments
                patterns = [p for p in patterns if p and not p.startswith('#')]
                if patterns:
                    path_specs[abs_path] = get_spec(patterns)
        except Exception as e:
            print(f"Error reading {local_gitignore}: {e}")
    
    # Cache the result
    spec_map[abs_path] = path_specs
    
    return path_specs

def load_gitignore_specs(root_path):
    """Load gitignore specs from all .gitignore files in the given path and its parent directories."""
    specs = {}
    pattern_lists = {}
    
    # Ensure we're working with absolute paths
    root_path = os.path.abspath(root_path)
    
    # Start with the root path and go up to find all parent .gitignore files
    current_path = root_path
    root_dir = os.path.dirname(current_path) if os.path.isfile(current_path) else current_path
    
    # Add default patterns to always ignore (like .git/)
    if DEFAULT_IGNORE_PATTERNS:
        # Use a special key for default patterns
        default_key = "DEFAULT"
        specs[default_key] = get_spec(DEFAULT_IGNORE_PATTERNS)
        pattern_lists[default_key] = DEFAULT_IGNORE_PATTERNS
    
    # Get all parent .gitignore files up to the filesystem root
    while current_path:
        gitignore_path = os.path.join(current_path, '.gitignore')
        
        # Check if file exists before trying to read it
        if os.path.isfile(gitignore_path):
            try:
                with open(gitignore_path, 'r') as f:
                    patterns = f.read().splitlines()
                    # Filter out empty lines and comments
                    patterns = [p for p in patterns if p and not p.startswith('#')]
                    if patterns:
                        print(f"Found .gitignore at {current_path}")
                        pattern_lists[current_path] = patterns
                        specs[current_path] = get_spec(patterns)
            except Exception as e:
                print(f"Error reading {gitignore_path}: {e}")
        
        # Move up one directory
        parent_path = os.path.dirname(current_path)
        if parent_path == current_path:  # Reached the filesystem root
            break
        current_path = parent_path
    
    return specs, root_dir

def is_ignored(path, gitignore_specs, root_dir):
    """Check if a path is ignored by any gitignore spec."""
    if not gitignore_specs:
        return False
    
    # Use the provided path without resolving again if it's already absolute
    if os.path.isabs(path):
        abs_path = path
    else:
        abs_path = os.path.abspath(path)
    
    # For debugging and pattern matching
    basename = os.path.basename(abs_path)
    
    # Special case for default patterns (always check first)
    if "DEFAULT" in gitignore_specs:
        # Get the relative path from the root directory
        rel_path = os.path.relpath(abs_path, root_dir)
        
        # Check if the path matches any default pattern
        if gitignore_specs["DEFAULT"].match_file(rel_path) or gitignore_specs["DEFAULT"].match_file(basename):
            # print(f"Ignoring {abs_path} due to default ignore pattern")
            return True
    
    # Check each spec, starting from the most specific (closest to the file)
    for dir_path, spec in sorted(gitignore_specs.items(), key=lambda x: len(x[0]) if x[0] != "DEFAULT" else 0, reverse=True):
        # Skip the default patterns as we've already checked them
        if dir_path == "DEFAULT":
            continue
            
        try:
            # Skip unnecessary path resolution - just use the paths directly for comparison
            # Only apply specs from directories that are parents of the path
            try:
                # For directory paths, ensure they're absolute for comparison
                if os.path.isabs(dir_path):
                    abs_dir_path = dir_path
                else:
                    abs_dir_path = os.path.abspath(dir_path)
                    
                if os.path.commonpath([abs_dir_path, abs_path]) == abs_dir_path:
                    # Get the relative path from the gitignore directory - no need to resolve again
                    rel_path = os.path.relpath(abs_path, abs_dir_path)
                    
                    # Check if the path matches any pattern in the spec
                    if spec.match_file(rel_path):
                        # print(f"Ignoring {abs_path} due to pattern in {dir_path}/.gitignore")
                        return True
                    
                    # Also check just the basename for directory patterns like "test_folder/"
                    if basename and spec.match_file(basename):
                        # print(f"Ignoring {abs_path} due to basename match in {dir_path}/.gitignore")
                        return True
                    
                    # Special handling for directory patterns like "test_folder/"
                    if os.path.isdir(abs_path):
                        # Try with trailing slash
                        dir_pattern = f"{basename}/"
                        if spec.match_file(dir_pattern):
                            # print(f"Ignoring directory {abs_path} due to pattern {dir_pattern} in {dir_path}/.gitignore")
                            return True
            except ValueError:
                # Skip if paths can't be compared (different drives, etc.)
                continue
        except Exception as e:
            print(f"Error checking if {path} is ignored: {e}")
            continue
    
    return False

def scan_directory(root_path, gitignore_specs=None, root_dir=None):
    """Scan a directory and return folders and files, respecting gitignore rules."""
    folders = []
    files = []
    
    # Ensure root_path is absolute
    root_path = os.path.abspath(root_path)
    
    # Initialize gitignore specs if not provided
    if gitignore_specs is None:
        gitignore_specs, root_dir = load_gitignore_specs(root_path)
    
    # Check if the directory itself is ignored
    if is_ignored(root_path, gitignore_specs, root_dir):
        print(f"Ignored: {root_path}")
        return {"folders": [], "files": []}
    
    # Get or update gitignore specs for this path
    gitignore_specs = get_spec_for_path(root_path, gitignore_specs)
    
    # Scan directory
    for entry in os.scandir(root_path):
        # Skip ignored files and folders
        if is_ignored(entry.path, gitignore_specs, root_dir):
            print(f"Ignored: {entry.name}")
            continue
        
        if entry.is_dir():
            folders.append(entry.name)
        else:
            files.append(entry.name)
    
    return {"folders": folders, "files": files, "gitignore_specs": gitignore_specs, "root_dir": root_dir}

if __name__ == "__main__":
    argparser = argparse.ArgumentParser(description="HelixDB Codebase Ingestion")
    argparser.add_argument("root", help="root directory of codebase", nargs="?", type=str, default=os.getcwd())
    args = argparser.parse_args()
    print(f"Scanning Codebase at: {args.root}\n")
    start_time = time.time()
    ingestion(args.root)
    print(f"\nInstance ID: {instance.instance_id}")
    print(f"Time taken: {time.time() - start_time}")