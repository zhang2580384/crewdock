FROM node:24-bookworm AS builder

WORKDIR /app
ARG VITE_GIT_SHA=unknown
ARG VITE_BUILD_TIME=unknown
ENV VITE_GIT_SHA=${VITE_GIT_SHA}
ENV VITE_BUILD_TIME=${VITE_BUILD_TIME}
COPY frontend/package*.json ./
RUN npm ci
COPY frontend ./
RUN npm run build

FROM nginx:1.27-alpine

COPY frontend/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=builder /app/dist /usr/share/nginx/html

EXPOSE 80
