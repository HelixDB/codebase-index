from tree_sitter import Language, Parser
import tree_sitter_python, tree_sitter_javascript
import os
import time
import argparse
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
    root_id = client.query('createRoot', {'name': root_path})[0]['root'][0]['id']
    populate(root_path, parent_id=root_id)

# Modifiable helper functions
# TODO: Replace with actual chunking function
def chunk_entity(text:str):
    return [text[i:i+50] for i in range(0, len(text)+1, 50)]

# TODO: Replace with actual embedding function
from random import random
def random_embedding(text:str):
    return [random() for _ in range(768)]

# Helper functions
def populate(full_path, curr_type='root', parent_id=None):
    dir_dict = scan_directory(full_path)

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
        populate(os.path.join(full_path, folder), curr_type='folder', parent_id=folder_id)

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

def scan_directory(root_path):
    folders = []
    files = []

    for entry in os.listdir(root_path):
        full_path = os.path.join(root_path, entry)
        if os.path.isdir(full_path):
            folders.append(entry)
        elif os.path.isfile(full_path):
            files.append(entry)

    return {
        "folders": folders,
        "files": files
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