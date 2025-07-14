export interface Model {
  name: string;
  modified_at: string;
  size: number;
}

export interface Message {
  role: 'user' | 'assistant';
  content: string;
  streaming?: boolean;
  id?: string;
  documents?: Document[];
}

export interface Document {
  id: string;
  name: string;
  size: number;
  type: string;
  path: string;
}

export interface LLMStats {
  tokensPerSecond: number;
  totalTokens: number;
  inputTokens: number;
  outputTokens: number;
  modelName?: string;
}

export interface ModelChatHistory {
  messages: Message[];
}

export interface msg {
    role: 'user' | 'assistant';
    content: string;
}