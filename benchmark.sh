#!/bin/bash
# GPU Performance Benchmark Runner for rust-code-mcp
#
# This script runs GPU performance benchmarks on the rust-code-mcp codebase

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}========================================${NC}"
echo -e "${BLUE}  rust-code-mcp GPU Benchmark Suite${NC}"
echo -e "${BLUE}========================================${NC}\n"

# Check if Qdrant is running
echo -e "${YELLOW}Checking Qdrant server...${NC}"
if curl -s http://localhost:6333/healthz > /dev/null 2>&1; then
    echo -e "${GREEN}✓ Qdrant is running${NC}\n"
else
    echo -e "${RED}✗ Qdrant is not running!${NC}"
    echo -e "${YELLOW}Please start Qdrant first:${NC}"
    echo -e "  docker run -p 6333:6333 qdrant/qdrant"
    exit 1
fi

# Check CUDA availability
echo -e "${YELLOW}Checking CUDA availability...${NC}"
if nvidia-smi > /dev/null 2>&1; then
    echo -e "${GREEN}✓ NVIDIA GPU detected${NC}"
    nvidia-smi --query-gpu=name,memory.total,memory.free --format=csv,noheader | \
        awk -F', ' '{printf "  GPU: %s\n  VRAM: %s total, %s free\n", $1, $2, $3}'
    echo ""
else
    echo -e "${YELLOW}⚠ No NVIDIA GPU detected or nvidia-smi not found${NC}"
    echo -e "${YELLOW}  GPU acceleration may not be available${NC}\n"
fi

# Build in release mode
echo -e "${YELLOW}Building in release mode...${NC}"
cargo build --release --quiet
echo -e "${GREEN}✓ Build complete${NC}\n"

# Run benchmarks
case "${1:-all}" in
    "gpu")
        echo -e "${BLUE}Running GPU Performance Benchmark...${NC}\n"
        cargo test --release benchmark_gpu_performance --ignored -- --nocapture
        ;;
    "compare")
        echo -e "${BLUE}Running Sequential vs Parallel Comparison...${NC}\n"
        cargo test --release benchmark_compare_sequential_vs_parallel --ignored -- --nocapture
        ;;
    "memory")
        echo -e "${BLUE}Running Memory Usage Benchmark...${NC}\n"
        cargo test --release benchmark_memory_usage --ignored -- --nocapture
        ;;
    "all")
        echo -e "${BLUE}Running All Benchmarks...${NC}\n"
        echo -e "${YELLOW}1/3: GPU Performance Benchmark${NC}"
        cargo test --release benchmark_gpu_performance --ignored -- --nocapture
        echo -e "\n${YELLOW}2/3: Sequential vs Parallel Comparison${NC}"
        cargo test --release benchmark_compare_sequential_vs_parallel --ignored -- --nocapture
        echo -e "\n${YELLOW}3/3: Memory Usage Benchmark${NC}"
        cargo test --release benchmark_memory_usage --ignored -- --nocapture
        ;;
    *)
        echo -e "${RED}Unknown benchmark: $1${NC}"
        echo -e "\nUsage: $0 [gpu|compare|memory|all]"
        echo -e "  gpu     - GPU performance benchmark"
        echo -e "  compare - Sequential vs parallel comparison"
        echo -e "  memory  - Memory usage analysis"
        echo -e "  all     - Run all benchmarks (default)"
        exit 1
        ;;
esac

echo -e "\n${GREEN}========================================${NC}"
echo -e "${GREEN}  Benchmarks Complete!${NC}"
echo -e "${GREEN}========================================${NC}\n"
