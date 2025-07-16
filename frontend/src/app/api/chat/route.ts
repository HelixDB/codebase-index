import { NextRequest, NextResponse } from 'next/server';
import dotenv from 'dotenv';
import * as fs from 'fs';
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";
import {msg} from "../../../lib/types";

import { GoogleGenAI , mcpToTool } from "@google/genai";
// import { Agent, run, MCPServerStreamableHttp } from '@openai/agents';
// import { Agent, ServerConfig } from '@huggingface/mcp-client';
// import { InferenceClient } from "@huggingface/inference";

dotenv.config();

const system_prompt = fs.readFileSync('./src/lib/instructions.txt', 'utf-8');

const mcp_url = 'http://localhost:8000/mcp';

const transport = new StreamableHTTPClientTransport(
    new URL(mcp_url),
);
const mcp_server = new McpServer({
    name: "codebase-indexing-server",
    version: "1.0.0"
});

// Opoen Router
// const OPEN_ROUTER_KEY = process.env.OPEN_ROUTER_KEY;
// await mcp_server.connect(transport);

// Huggingface
// const HF_TOKEN = process.env.HF_TOKEN;
// const hf_server_configs: ServerConfig = {
//     type: "http",
//     config: {
//         url: mcp_url,
//     }
// };
// const hf_agent = new Agent({
//     provider: "groq",
//     apiKey: HF_TOKEN,
//     model: "moonshotai/Kimi-K2-Instruct",
//     prompt: system_prompt,
//     servers: [
//         hf_server_configs
//     ]
// });

// await hf_agent.loadTools();


// Gemini
const GEMINI_API_KEY = process.env.GEMINI_API_KEY;
const geminiClient = new GoogleGenAI({ apiKey: GEMINI_API_KEY });
const mcp_client = new Client(
    {
      name: "codebase-indexing-client",
      version: "1.0.0"
    }
);
await mcp_client.connect(transport);

// OpenAI Agents
// const openai_mcp_server = new MCPServerStreamableHttp({
//     url: mcp_url,
//     name: "codebase-indexing-client",
// })

// const agent = new Agent({
//     name: 'Creative writer',
//     model: 'gpt-4.1-mini-2025-04-14',
//     instructions: system_prompt,
//     mcpServers: [openai_mcp_server]
// });

// await openai_mcp_server.connect();

export async function POST(req: NextRequest) {
    const messagesData = await req.json();
    
    // OpenAI Agents
    // const lastUserMessage = messagesData.messages.filter((msg: msg) => msg.role === 'user').pop();
    // console.log('Sending to OpenAI:', lastUserMessage);

    // Gemini
    const formattedContents = messagesData.messages.map((msg: msg) => ({
        role: msg.role === 'user' ? 'user' : 'model',
        parts: [{ text: msg.content }]
    }));

    // Huggingface & OpenRouter
    // const formattedContents = messagesData.messages.map((msg: msg) => ({
    //     role: msg.role === 'user' ? 'user' : 'assistant',
    //     content: msg.content
    // }));

    try {
        /// Gemini
        const response = await geminiClient.models.generateContentStream({
            model: "gemini-2.5-pro",
            contents: formattedContents,
            config: {
                systemInstruction: system_prompt,
                tools: [mcpToTool(mcp_client)],
                thinkingConfig: {
                    thinkingBudget: -1,
                    // Turn off thinking: 0
                    // Turn on dynamic thinking: -1
                },
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

        /// OpenRouter
        // const response = await fetch('https://openrouter.ai/api/v1/chat/completions', {
        //     method: 'POST',
        //     headers: {
        //       Authorization: `Bearer ${OPEN_ROUTER_KEY}`,
        //       'Content-Type': 'application/json',
        //     },
        //     body: JSON.stringify({
        //       model: 'moonshotai/kimi-k2:free',
        //       messages: [{role: 'system', content: system_prompt}, ...formattedContents],
        //       tools: [],
        //       stream: true,
        //     }),
        // });

        // const stream = new ReadableStream({
        //     async start(controller) {
        //         try {
        //             if (!response.body) {
        //                 throw new Error('Response body is null');
        //             }
                    
        //             const reader = response.body.getReader();
        //             const decoder = new TextDecoder();
                    
        //             while (true) {
        //                 const { done, value } = await reader.read();
        //                 if (done) break;
                        
        //                 // Decode the chunk and add to buffer
        //                 const line = decoder.decode(value, { stream: true });
        //                 if (!line.includes('data: ')) {
        //                     console.log(line);
        //                 }
                        
        //                 if (line.includes('data: [DONE]') || line.includes(': OPENROUTER PROCESSING')) continue;
                            
        //                 // Extract the JSON data from the line
        //                 if (line.startsWith('data: ')) {
        //                     try {
        //                         const jsonData = JSON.parse(line.substring(6));
        //                         if (jsonData.choices && jsonData.choices[0]) {
        //                             const content = jsonData.choices[0].delta?.content || '';
        //                             if (content) {
        //                                 controller.enqueue(new TextEncoder().encode(content));
        //                             }
        //                         }
        //                     } catch (e) {
        //                         console.error('Error parsing JSON:', e, line);
        //                     }
        //                 }
        //             }

        //             controller.close();
        //         } catch (error) {
        //             console.error('Stream error:', error);
        //             controller.error(error);
        //         }
        //     }
        // });

        /// Huggingface
        // const response = await hf_agent.run(formattedContents);
        
        // const stream = new ReadableStream({
        //     async start(controller) {
        //         try {
        //             for await (const chunk of response) {
        //                 if ('choices' in chunk) {
        //                     for (const choice of chunk.choices) {
        //                         controller.enqueue(new TextEncoder().encode(choice.delta.content));
        //                         controller.enqueue(new TextEncoder().encode("\n"));
        //                     }
        //                 }
        //             }
        //             controller.close();
        //         } catch (error) {
        //             console.error('Stream error:', error);
        //             controller.error(error);
        //         }
        //     }
        // });


        /// OPENAI AGENTS
        // const response = await run(
        //     agent,
        //     [{
        //         role: 'user',
        //         content: lastUserMessage?.content || ''
        //     }],
        //     {
        //         stream: true
        //     }
        // );

        // console.log(response.history);

        // const stream = new ReadableStream({
        //     async start(controller) {
        //         try {
        //             for await (const chunk of response) {
        //                 if (chunk.type == "raw_model_stream_event") {
        //                     const data = chunk.data;
        //                     if (data.type == 'model' && data.event.type == 'response.output_text.delta') {
        //                         controller.enqueue(new TextEncoder().encode(data.event.delta));
        //                     }
        //                 }
        //             }
        //             controller.close();
        //         } catch (error) {
        //             console.error('Stream error:', error);
        //             controller.error(error);
        //         }
        //     }
        // });

        return new NextResponse(stream, {
            headers: {
                'Content-Type': 'text/plain; charset=utf-8',
                'Transfer-Encoding': 'chunked'
            }
        });
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    } catch (error: any) {
        console.error('API error:', error);
        return NextResponse.json({ error: error.message || 'Unknown error' }, { status: 500 });
    }
}
