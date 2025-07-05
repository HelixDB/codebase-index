// Create Root
QUERY createRoot(name: String) => 
    root <- AddN<Root>({name:name})
    RETURN root

// Create Folders
QUERY createSuperFolder(root_id: ID, name: String) => 
    root <- N<Root>(root_id)
    folder <- AddN<Folder>({name:name})
    AddE<Root_to_Folder>()::From(root)::To(folder)
    RETURN folder

QUERY createSubFolder(folder_id: ID, name: String) => 
    folder <- N<Folder>(folder_id)
    subfolder <- AddN<Folder>({name:name})
    AddE<Folder_to_Folder>()::From(folder)::To(subfolder)
    RETURN subfolder

// Create Files
QUERY createSuperFile(root_id: ID, name: String, text: String) => 
    root <- N<Root>(root_id)
    file <- AddN<File>({name:name, text:text})
    AddE<Root_to_File>()::From(root)::To(file)
    RETURN file

QUERY createFile(folder_id: ID, name: String, text: String) => 
    folder <- N<Folder>(folder_id)
    file <- AddN<File>({name:name, text:text})
    AddE<Folder_to_File>()::From(folder)::To(file)
    RETURN file

// Create Entities
QUERY createSuperEntity(file_id: ID, type: String, start_byte: I64, end_byte: I64, order: I64, text: String) => 
    file <- N<File>(file_id)
    entity <- AddN<Entity>({type:type, start_byte:start_byte, end_byte:end_byte, order:order, text:text})
    AddE<File_to_Entity>()::From(file)::To(entity)
    RETURN entity

QUERY embedSuperEntity(entity_id: ID, vector: [F64]) => 
    entity <- N<Entity>(entity_id)
    embeded_code <- AddV<EmbededCode>(vector)
    AddE<Entity_to_EmbededCode>()::From(entity)::To(embeded_code)
    RETURN embeded_code

QUERY createSubEntity(entity_id: ID, type: String, start_byte: I64, end_byte: I64, order: I64, text: String) => 
    parent <- N<Entity>(entity_id)
    entity <- AddN<Entity>({type:type, start_byte:start_byte, end_byte:end_byte, order:order, text:text})
    AddE<Entity_to_Entity>()::From(parent)::To(entity)
    RETURN entity



// Get Root
QUERY getRoot() => 
    root <- N<Root>
    RETURN root

QUERY getFolderRoot(folder_id: ID) => 
    root <- N<Folder>(folder_id)::In<Root_to_Folder>
    RETURN root

QUERY getFileRoot(file_id: ID) => 
    root <- N<File>(file_id)::In<Root_to_File>
    RETURN root

// Get Folders
QUERY getFolder(folder_id: ID) => 
    folder <- N<Folder>(folder_id)
    RETURN folder

QUERY getRootFolders(root_id: ID) => 
    folders <- N<Root>(root_id)::Out<Root_to_Folder>
    RETURN folders

QUERY getSuperFolders(folder_id: ID) => 
    folders <- N<Folder>(folder_id)::In<Folder_to_Folder>
    RETURN folders

QUERY getSubFolders(folder_id: ID) => 
    subfolders <- N<Folder>(folder_id)::Out<Folder_to_Folder>
    RETURN subfolders

QUERY getFileFolder(file_id: ID) => 
    folder <- N<File>(file_id)::In<Folder_to_File>
    RETURN folder

// Get Files
QUERY getFile(file_id: ID) => 
    file <- N<File>(file_id)
    RETURN file

QUERY getRootFiles(root_id: ID) => 
    files <- N<Root>(root_id)::Out<Root_to_File>
    RETURN files

QUERY getFolderFiles(folder_id: ID) => 
    files <- N<Folder>(folder_id)::Out<Folder_to_File>
    RETURN files

// Get Entity
QUERY getFileEntities(file_id: ID) => 
    entity <- N<File>(file_id)::Out<File_to_Entity>
    RETURN entity

QUERY getEntityFile(entity_id: ID) => 
    file <- N<Entity>(entity_id)::In<File_to_Entity>
    RETURN file

QUERY searchSuperEntity(vector: [F64], k: I64) => 
    vectors <- SearchV<EmbededCode>(vector, k)
    RETURN vectors

QUERY getSubEntities(entity_id: ID) => 
    entities <- N<Entity>(entity_id)::Out<Entity_to_Entity>
    RETURN entities

QUERY getSuperEntity(entity_id: ID) => 
    entity <- N<Entity>(entity_id)::In<Entity_to_Entity>
    RETURN entity