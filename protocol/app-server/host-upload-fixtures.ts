// Generated contract examples; not admitted management messages.
import type {HostUploadEnvelope} from './v80.js';
export const hostUploads = [
  {
    "jsonrpc": "2.0",
    "id": 1,
    "method": "item/tool/result/begin",
    "params": {
      "candidate": {
        "callbackIncarnation": "server-1",
        "requestId": "server-1:42",
        "threadId": "conv_36524fd8-f674-7fc2-8125-06d01fee0e19",
        "turnId": "turn-1",
        "callId": "call-1",
        "connectionIncarnation": "host-1",
        "subscriptionIncarnation": "subscription-1",
        "uploadId": "00000000-0000-4000-8000-000000000042"
      },
      "success": true,
      "items": [
        {
          "typeTag": 1,
          "length": "8"
        }
      ]
    }
  },
  {
    "jsonrpc": "2.0",
    "id": 2,
    "method": "item/tool/result/chunk",
    "params": {
      "candidate": {
        "callbackIncarnation": "server-1",
        "requestId": "server-1:42",
        "threadId": "conv_36524fd8-f674-7fc2-8125-06d01fee0e19",
        "turnId": "turn-1",
        "callId": "call-1",
        "connectionIncarnation": "host-1",
        "subscriptionIncarnation": "subscription-1",
        "uploadId": "00000000-0000-4000-8000-000000000042"
      },
      "itemIndex": 0,
      "sequence": "1",
      "offset": "0",
      "dataBase64": "cmVjZWlwdAo=",
      "sha256": "6afd9c1cbf495f9221fe5f9d03d4f0e962babd83fd293c696e69686510611a63"
    }
  },
  {
    "jsonrpc": "2.0",
    "id": 3,
    "method": "item/tool/result/end",
    "params": {
      "candidate": {
        "callbackIncarnation": "server-1",
        "requestId": "server-1:42",
        "threadId": "conv_36524fd8-f674-7fc2-8125-06d01fee0e19",
        "turnId": "turn-1",
        "callId": "call-1",
        "connectionIncarnation": "host-1",
        "subscriptionIncarnation": "subscription-1",
        "uploadId": "00000000-0000-4000-8000-000000000042"
      },
      "success": true,
      "items": [
        {
          "typeTag": 1,
          "length": "8"
        }
      ],
      "total": "8",
      "digest": "8078e771640c9054763b976a9762ccc7e02375691f04c6feb76895b50095c423"
    }
  },
  {
    "jsonrpc": "2.0",
    "id": 4,
    "method": "item/tool/result/abort",
    "params": {
      "candidate": {
        "callbackIncarnation": "server-1",
        "requestId": "server-1:42",
        "threadId": "conv_36524fd8-f674-7fc2-8125-06d01fee0e19",
        "turnId": "turn-1",
        "callId": "call-1",
        "connectionIncarnation": "host-1",
        "subscriptionIncarnation": "subscription-1",
        "uploadId": "00000000-0000-4000-8000-000000000042"
      }
    }
  }
] satisfies HostUploadEnvelope[];
