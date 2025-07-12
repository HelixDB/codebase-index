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
    async with mcp_client:
        response = await gemini_client.aio.models.generate_content_stream(
            model="gemini-2.5-flash-lite-preview-06-17",
            # query below:
            contents="",
            config=genai.types.GenerateContentConfig(
                # system prompt below:
                system_instruction="""""",
                temperature=0.2,
                tools=[mcp_client.session],
            ),
        )
        
        async for chunk in response:
            print(chunk.text, end="")


if __name__ == "__main__":
    asyncio.run(main())
