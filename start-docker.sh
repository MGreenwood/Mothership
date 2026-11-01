#!/bin/bash

echo "Starting Mothership Docker Services..."

# Check if .env file exists
if [ ! -f .env ]; then
    echo ".env file not found! Please create a .env file with your OAuth credentials."
    echo "Required variables:"
    echo "  GOOGLE_CLIENT_ID=your-google-client-id"
    echo "  GOOGLE_CLIENT_SECRET=your-google-client-secret"
    echo "  JWT_SECRET=your-jwt-secret"
    echo "  ADMIN_SECRET=your-admin-secret"
    exit 1
fi

# Stop any existing containers
echo "Stopping existing containers..."
docker-compose -f docker-compose.dev.yml down

# Build and start services
echo "Building and starting services..."
docker-compose -f docker-compose.dev.yml up --build -d

# Wait for services to be healthy
echo "Waiting for services to be ready..."
sleep 10

# Check service status
echo "Service Status:"
docker-compose -f docker-compose.dev.yml ps

# Show logs
echo "Recent logs:"
docker-compose -f docker-compose.dev.yml logs --tail=20

echo ""
echo "Mothership services are running!"
echo "Mothership Server: http://localhost:7523"
echo "Auth Server: http://localhost:3001"
echo ""
echo "To stop services: docker-compose -f docker-compose.dev.yml down"
echo "To view logs: docker-compose -f docker-compose.dev.yml logs -f" 