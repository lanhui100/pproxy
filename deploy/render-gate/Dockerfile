# render-gate 通用容器镜像（Render/Koyeb/ClawCloud Run/任意 Docker 平台通用）
FROM node:20-alpine

WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci --omit=dev --no-audit --no-fund
COPY server.js ./

ENV NODE_ENV=production
# 平台注入 PORT；缺省 3000
EXPOSE 3000
CMD ["node", "server.js"]
