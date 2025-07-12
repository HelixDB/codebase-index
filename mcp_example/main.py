from fastmcp import Client
from fastmcp.client.auth import BearerAuth
from google import genai
import asyncio
import os
import dotenv

dotenv.load_dotenv(override=True)

GEMINI_API_KEY = os.getenv("GEMINI_API_KEY")
# IN_PROD = os.getenv("IN_PROD")
# PROD_URL = os.getenv("PROD_URL")

gemini_client = genai.Client(api_key=GEMINI_API_KEY)

async def main():
    TOKEN = os.getenv("API_SECRET_TOKEN")
    mcp_client = Client(
        "http://localhost:8000/mcp/",
        # auth=BearerAuth(TOKEN)
    )

    prompt = "im super duper confused, what parser languages are in ingestion.py?"

    async with mcp_client:
        response = await gemini_client.aio.models.generate_content_stream(
            model="gemini-2.5-flash-lite-preview-06-17",
            # query below:
            contents=prompt,
            config=genai.types.GenerateContentConfig(
                # system prompt below:
                system_instruction="""
                You are a helpful assistant that can answer questions about the codebase. Please use the tools provided to answer the question.
                
                Available endpoints:
                getRoot - Parameters: None - Returns the root node of the codebase
                getFolderRoot - Parameters: `folder_id` (string) - Returns the root node of a specific folder
                getFileRoot - Parameters: `file_id` (string) - Returns the root node of a specific file
                getFolder - Parameters: `folder_id` (string) - Returns a specific folder
                getRootFolders - Parameters: `root_id` (string) - Returns the folders under a root
                getSuperFolders - Parameters: `folder_id` (string) - Returns the parent folders of a folder
                getSubFolders - Parameters: `folder_id` (string) - Returns the subfolders of a folder
                getFileFolder - Parameters: `file_id` (string) - Returns the folder containing a file
                getFile - Parameters: `file_id` (string) - Returns a specific file
                getRootFiles - Parameters: `root_id` (string) - Returns the files under a root
                getFolderFiles - Parameters: `folder_id` (string) - Returns the files in a folder
                getFileEntities - Parameters: `file_id` (string) - Returns the entities in a file
                getEntityFile - Parameters: `entity_id` (string) - Returns the file containing an entity
                getSubEntities - Parameters: `entity_id` (string) - Returns the child entities of an entity
                getSuperEntity - Parameters: `entity_id` (string) - Returns the parent entity of an entity

                **For Semantic Searches:**
                For example queries like: "what are the functions in ingestion.py?" you should use the semantic_search_code tool.
                - `semantic_search_code` - Parameters: `query` (string), `k` (integer, optional, default=5) - Returns the entities by embedding vector

                """,
                temperature=0.2,
                tools=[mcp_client.session],
            ),
        )
        
        async for chunk in response:
            print(chunk.text, end="")


if __name__ == "__main__":
    asyncio.run(main())
