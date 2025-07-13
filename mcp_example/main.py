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

    with open('instructions.txt', 'r') as f:
        system_prompt = f.read()

    prompt = "What are the parameters of the function process_entity in RUST? What is its file name and parent folder name? Also, what is the root folder name?"
    
    async with mcp_client:
        response = await gemini_client.aio.models.generate_content_stream(
            model="gemini-2.5-flash",
            # query below:
            contents=prompt,
            config=genai.types.GenerateContentConfig(
                # system prompt below:
                system_instruction=system_prompt,
                temperature=0.2,
                tools=[mcp_client.session],
            ),
        )
        
        async for chunk in response:
            try:
                # Check if candidates exist and have content
                if chunk.candidates and len(chunk.candidates) > 0 and chunk.candidates[0].content:
                    for part in chunk.candidates[0].content.parts:
                        if part.text:
                            print(part.text, end="")
                else:
                    raise Exception(chunk.candidates[0].finish_reason)
            except Exception as e:
                print(f"Error processing chunk: {e}")

        print()


if __name__ == "__main__":
    asyncio.run(main())
