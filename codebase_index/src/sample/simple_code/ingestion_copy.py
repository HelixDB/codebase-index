from tree_sitter import Language, Parser
import tree_sitter_python, tree_sitter_javascript
import os
import time
import argparse
import pathspec
from helix import Client, Instance

# Parser Languages
PY_LANGUAGE = Language(tree_sitter_python.language())
JS_LANGUAGE = Language(tree_sitter_javascript.language())

# Parsers
py_parser = Parser(PY_LANGUAGE)
js_parser = Parser(JS_LANGUAGE)

# HelixDB Instance
instance = Instance()
time.sleep(1)

# HelixDB Client
client = Client(local=True, verbose=False)

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
    return [text[i:i+50] for i in range(0, len(text)+1, 50)]

# TODO: Replace with actual embedding function
from random import random
def random_embedding(text:str):
    return [random() for _ in range(768)]

# Helper functions
def populate(full_path, curr_type='root', parent_id=None, gitignore_specs=None, root_dir=None):
    dir_dict = scan_directory(full_path, gitignore_specs, root_dir)
    
    # Extract gitignore specs and root_dir if they were returned by scan_directory
    gitignore_specs = dir_dict.get("gitignore_specs", gitignore_specs)
    root_dir = dir_dict.get("root_dir", root_dir)

    print(f'Processing {len(dir_dict["folders"])} folders')
    for folder in dir_dict["folders"]:
        print(f"Reached {folder}")
        if curr_type == 'root':
            # Create super folder
            folder_id = client.query('createSuperFolder', {'root_id': parent_id, 'name': folder})[0]['folder'][0]['id']
        else:
            # Create sub folder
            folder_id = client.query('createSubFolder', {'folder_id': parent_id, 'name': folder})[0]['subfolder'][0]['id']
        
        # Recursively populate the stuff in the folder
        populate(os.path.join(full_path, folder), curr_type='folder', parent_id=folder_id, gitignore_specs=gitignore_specs, root_dir=root_dir)

    print(f'Processing {len(dir_dict["files"])} files')
    for file in dir_dict["files"]:
        print(f"Reached {file}")
        if file.endswith('.py'):
            # Extract python code structure with tree-sitter
            file_path = os.path.join(full_path, file)
            tree, code = parse_file(file_path, py_parser)

            if tree:
                tree_dict = node_to_dict(tree.root_node, code, 0)
                del tree
                del code

                if curr_type == 'root':
                    # Create super file
                    file_id = client.query('createSuperFile', {'root_id': parent_id, 'name': file, 'text': tree_dict['text']})[0]['file'][0]['id']
                else:
                    # Create sub file
                    file_id = client.query('createFile', {'folder_id': parent_id, 'name': file, 'text': tree_dict['text']})[0]['file'][0]['id']

                children = tree_dict['children']
                del tree_dict

                print(f"Processing {len(children)} super entities")
                for superentity in children:
                    print(f"Reached {superentity['type']}")
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
            else:
                print(f'Failed to parse file: {file}')
                del tree
                del code
        else:
            print(f'Not python file: {file}')

    del dir_dict

def process_entities(parent_dict, parent_id):
    children = parent_dict['children']
    for entity in children:
        # Create sub entity
        entity_id = client.query('createSubEntity', {'entity_id': parent_id, 'entity_type': entity['type'], 'start_byte': entity['start_byte'], 'end_byte': entity['end_byte'], 'order': entity['order'], 'text': entity['text']})[0]['entity'][0]['id']
        
        # Recursively process sub entities in the entity
        process_entities(entity, entity_id)

def parse_file(file_path, parser):
    try:
        with open(file_path, 'rb') as file:
            source_code = file.read()

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

def load_gitignore_specs(root_path):
    """Load gitignore specs from all .gitignore files in the given path and its parent directories."""
    specs = {}
    
    # Ensure we're working with absolute paths
    root_path = os.path.abspath(root_path)
    
    # Start with the root path and go up to find all parent .gitignore files
    current_path = root_path
    root_dir = os.path.dirname(current_path) if os.path.isfile(current_path) else current_path
    
    # Get all parent .gitignore files up to the filesystem root
    while current_path:
        gitignore_path = os.path.join(current_path, '.gitignore')
        if os.path.isfile(gitignore_path):
            try:
                with open(gitignore_path, 'r') as f:
                    patterns = f.read().splitlines()
                    # Filter out empty lines and comments
                    patterns = [p for p in patterns if p and not p.startswith('#')]
                    if patterns:
                        print(f"Found .gitignore at {current_path} with patterns: {patterns}")
                        specs[current_path] = pathspec.PathSpec.from_lines('gitwildmatch', patterns)
            except Exception as e:
                print(f"Error reading {gitignore_path}: {e}")
        
        # Move up one directory
        parent_path = os.path.dirname(current_path)
        if parent_path == current_path:  # Reached the filesystem root
            break
        current_path = parent_path
    
    return specs, root_dir

def is_ignored(path, gitignore_specs):
    if not gitignore_specs:
        return False
    
    # Ensure path is absolute
    abs_path = os.path.abspath(path)
    
    # For debugging
    basename = os.path.basename(abs_path)
    
    # Check each spec, starting from the most specific (closest to the file)
    for dir_path, spec in sorted(gitignore_specs.items(), key=lambda x: len(x[0]), reverse=True):
        try:
            # Ensure both paths are absolute before comparing
            abs_dir_path = os.path.abspath(dir_path)
            
            # Only apply specs from directories that are parents of the path
            if os.path.commonpath([abs_dir_path, abs_path]) == abs_dir_path:
                # Get the relative path from the gitignore directory
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
            # Skip if paths can't be compared
            continue
    
    return False

def scan_directory(root_path, gitignore_specs=None, root_dir=None):
    folders = []
    files = []
    
    # Ensure root_path is absolute
    root_path = os.path.abspath(root_path)
    
    # Initialize gitignore specs if not provided
    if gitignore_specs is None:
        gitignore_specs, root_dir = load_gitignore_specs(root_path)
        print(f"Loaded gitignore specs from {len(gitignore_specs)} files")
        for path in gitignore_specs:
            print(f"  - {path}/.gitignore")
    
    # Check if the directory itself is ignored
    if is_ignored(root_path, gitignore_specs):
        print(f"Directory {root_path} is ignored by gitignore rules")
        return {"folders": [], "files": []}
    
    # Check for a .gitignore file in the current directory
    local_gitignore = os.path.join(root_path, '.gitignore')
    if os.path.isfile(local_gitignore):
        # Check if this specific path is already in gitignore_specs
        if root_path not in gitignore_specs:
            try:
                with open(local_gitignore, 'r') as f:
                    patterns = f.read().splitlines()
                    # Filter out empty lines and comments
                    patterns = [p for p in patterns if p and not p.startswith('#')]
                    if patterns:
                        gitignore_specs[root_path] = pathspec.PathSpec.from_lines('gitwildmatch', patterns)
            except Exception as e:
                print(f"Error reading {local_gitignore}: {e}")
    
    for entry in os.listdir(root_path):
        full_path = os.path.join(root_path, entry)
        
        # Skip if the path is ignored by gitignore rules
        if is_ignored(full_path, gitignore_specs):
            print(f"Skipping {entry} as it's ignored by gitignore rules")
            continue
            
        if os.path.isdir(full_path):
            folders.append(entry)
        elif os.path.isfile(full_path):
            files.append(entry)
    
    return {
        "folders": folders,
        "files": files,
        "gitignore_specs": gitignore_specs,
        "root_dir": root_dir
    }

if __name__ == "__main__":
    argparser = argparse.ArgumentParser(description="HelixDB Codebase Ingestion")
    argparser.add_argument("root", help="root directory of codebase", nargs="?", type=str, default=os.getcwd())
    args = argparser.parse_args()
    print(f"Scanning Codebase at: {args.root}")
    start_time = time.time()
    ingestion(args.root)
    print(f"Instance ID: {instance.instance_id}")
    print(f"Time taken: {time.time() - start_time}")