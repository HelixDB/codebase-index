from fastmcp import Client
from fastmcp.client.auth import BearerAuth
from google import genai
import asyncio
import os
import dotenv
from rich import print

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

    title = """
$$\   $$\           $$\ $$\           $$$$$$\                 $$\                                         
$$ |  $$ |          $$ |\__|          \_$$  _|                $$ |                                        
$$ |  $$ | $$$$$$\  $$ |$$\ $$\   $$\   $$ |  $$$$$$$\   $$$$$$$ | $$$$$$\  $$\   $$\  $$$$$$\   $$$$$$\  
$$$$$$$$ |$$  __$$\ $$ |$$ |\$$\ $$  |  $$ |  $$  __$$\ $$  __$$ |$$  __$$\ \$$\ $$  |$$  __$$\ $$  __$$\ 
$$  __$$ |$$$$$$$$ |$$ |$$ | \$$$$  /   $$ |  $$ |  $$ |$$ /  $$ |$$$$$$$$ | \$$$$  / $$$$$$$$ |$$ |  \__|
$$ |  $$ |$$   ____|$$ |$$ | $$  $$<    $$ |  $$ |  $$ |$$ |  $$ |$$   ____| $$  $$<  $$   ____|$$ |      
$$ |  $$ |\$$$$$$$\ $$ |$$ |$$  /\$$\ $$$$$$\ $$ |  $$ |\$$$$$$$ |\$$$$$$$\ $$  /\$$\ \$$$$$$$\ $$ |      
\__|  \__| \_______|\__|\__|\__/  \__|\______|\__|  \__| \_______| \_______|\__/  \__| \_______|\__|
"""

    async with mcp_client:
        print(title)
        print("Type 'exit' to quit\n")
        history = []
        
        while True:
            try:
                # Get user input
                user_input = input("You: ")
                
                # Check for exit command
                if user_input.lower() in ['exit', 'quit', 'q']:
                    break
                
                # Skip empty input
                if not user_input.strip():
                    continue
                
                # Prepare the prompt with user input
                messages = history + [{
                    "role": "user",
                    "parts": [{"text": user_input}]
                }]
                
                # Get and stream the response
                print("\nAssistant: ", end="")
                response = await gemini_client.aio.models.generate_content_stream(
                    model="gemini-2.5-pro",
                    contents=messages,
                    config=genai.types.GenerateContentConfig(
                        system_instruction=system_prompt,
                        temperature=0.2,
                        tools=[mcp_client.session],
                        thinking_config=genai.types.ThinkingConfig(thinking_budget=-1)
                    ),
                )
                
                llm_response = ""
                async for chunk in response:
                    try:
                        if chunk.candidates and len(chunk.candidates) > 0 and chunk.candidates[0].content:
                            for part in chunk.candidates[0].content.parts:
                                if part.text:
                                    print(part.text, end="")
                                    llm_response += part.text
                        else:
                            raise Exception(chunk.candidates[0].finish_reason)
                    except Exception as e:
                        print(f"\nError: {e}")
                
                print("\n")  # Add spacing between exchanges
                
                history.append({"role": "user", "parts": [{"text": user_input}]})
                history.append({"role": "model", "parts": [{"text": llm_response}]})
                
            except Exception as e:
                print(f"\nAn error occurred: {e}")
                continue


if __name__ == "__main__":
    asyncio.run(main())
