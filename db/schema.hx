N::Root {
    name: String,
    extracted_at: Date DEFAULT NOW
}

N::Folder {
    name: String,
    extracted_at: Date DEFAULT NOW
}

N::File {
    name: String,
    extension: String,
    text: String,
    extracted_at: Date DEFAULT NOW
}

N::Entity {
    entity_type: String,
    start_byte: I64,
    end_byte: I64,
    order: I64,
    text: String,
    extracted_at: Date DEFAULT NOW
}

E::Root_to_Folder {
    From: Root,
    To: Folder,
    Properties: {
    }
}

E::Root_to_File {
    From: Root,
    To: File,
    Properties: {
    }
}

E::Folder_to_Folder {
    From: Folder,
    To: Folder,
    Properties: {
    }
}

E::Folder_to_File {
    From: Folder,
    To: File,
    Properties: {
    }
}

E::File_to_Entity {
    From: File,
    To: Entity,
    Properties: {
    }
}

E::Entity_to_Entity {
    From: Entity,
    To: Entity,
    Properties: {
    }
}

E::Entity_to_EmbededCode {
    From: Entity,
    To: EmbededCode,
    Properties: {
    }
}

V::EmbededCode {
    vector: [F64]
}