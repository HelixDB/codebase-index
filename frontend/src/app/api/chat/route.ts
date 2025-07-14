import { NextRequest, NextResponse } from 'next/server';
import dotenv from 'dotenv';
import { GoogleGenAI , mcpToTool } from "@google/genai";
import * as fs from 'fs';
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";

dotenv.config();

const GEMINI_API_KEY = process.env.GEMINI_API_KEY;
const geminiClient = new GoogleGenAI({ apiKey: GEMINI_API_KEY });

const serverParams = new StdioClientTransport({
    command: "uv", 
    args: ["run", "./src/mcp/server.py"]
  });

const system_prompt = fs.readFileSync('./src/lib/instructions.txt', 'utf-8');

const mcp_client = new Client(
    {
      name: "codebase-indexing-client",
      version: "1.0.0"
    }
  );

export async function POST(req: NextRequest) {
    const messagesData = await req.json();

    await mcp_client.connect(serverParams);
    
    const formattedContents = messagesData.messages.map((msg: any) => ({
        role: msg.role === 'user' ? 'user' : 'model',
        parts: [{ text: msg.content }]
    }));

    try {
        const response = await geminiClient.models.generateContentStream({
            model: "gemini-2.5-flash",
            contents: formattedContents,
            config: {
                systemInstruction: system_prompt,
                tools: [mcpToTool(mcp_client)]
            },
        });

        const stream = new ReadableStream({
            async start(controller) {
                try {
                    for await (const chunk of response) {
                        const text = chunk.candidates?.[0]?.content?.parts?.[0]?.text || '';
                        controller.enqueue(new TextEncoder().encode(text));
                    }
                    controller.close();
                } catch (error) {
                    console.error('Stream error:', error);
                    controller.error(error);
                }
            }
        });

        return new NextResponse(stream, {
            headers: {
                'Content-Type': 'text/plain; charset=utf-8',
                'Transfer-Encoding': 'chunked'
            }
        });
    } catch (error: any) {
        console.error('API error:', error);
        return NextResponse.json({ error: error.message || 'Unknown error' }, { status: 500 });
    }
}
